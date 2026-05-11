use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Datelike, Utc};
use poise::serenity_prelude::{self as serenity, HttpError};
use sqlx::SqlitePool;
use tracing::{error, info, warn};

use crate::{
    config::Config,
    db::{
        DecisionStatus, OpenCriticSubscription, delete_opencritic_subscription,
        insert_opencritic_announcement, insert_opencritic_decision,
        list_opencritic_subscriptions, opencritic_announcement_exists,
        opencritic_decision_exists,
    },
    opencritic::{OpenCriticClient, OpenCriticGame},
};

fn scan_years() -> Vec<i32> {
    let current_year = Utc::now().year();
    vec![current_year, current_year - 1]
}

const DISCORD_UNKNOWN_CHANNEL_ERROR_CODE: isize = 10003;

fn is_channel_gone_error(err: &serenity::Error) -> bool {
    matches!(
        err,
        serenity::Error::Http(HttpError::UnsuccessfulRequest(resp))
            if resp.error.code == DISCORD_UNKNOWN_CHANNEL_ERROR_CODE
    )
}

#[derive(Debug, Clone)]
pub struct PollOutcome {
    pub scanned_games: usize,
    pub active_subscriptions: usize,
    pub messages_sent: usize,
    pub already_decided: usize,
    pub pages_requested: usize,
    pub rate_limit_remaining: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LastPollSnapshot {
    pub at: DateTime<Utc>,
    pub outcome: PollOutcome,
}

pub async fn run_opencritic_poll(
    pool: &SqlitePool,
    client: &OpenCriticClient,
    http: &serenity::Http,
) -> Result<PollOutcome> {
    let subscriptions = list_opencritic_subscriptions(pool).await?;

    if subscriptions.is_empty() {
        return Ok(PollOutcome {
            scanned_games: 0,
            active_subscriptions: 0,
            messages_sent: 0,
            already_decided: 0,
            pages_requested: 0,
            rate_limit_remaining: client.last_rate_limit_remaining(),
        });
    }

    let years = scan_years();

    let min_threshold = subscriptions
        .iter()
        .map(|s| s.threshold)
        .fold(f64::INFINITY, f64::min);
    let min_reviews = subscriptions
        .iter()
        .map(|s| s.min_reviews)
        .min()
        .expect("subscriptions is non-empty");

    let scan = client
        .scan_years(&years, min_threshold, min_reviews)
        .await?;

    let now = Utc::now().to_rfc3339();
    let mut messages_sent = 0;
    let mut already_decided = 0;
    let mut removed_channels: HashSet<String> = HashSet::new();

    for game in &scan.games {
        let mut fetched_game: Option<OpenCriticGame> = None;

        for subscription in &subscriptions {
            if removed_channels.contains(&subscription.channel_id) {
                continue;
            }
            if !game.qualifies(subscription.threshold, subscription.min_reviews) {
                continue;
            }
            if opencritic_decision_exists(pool, game.id, &subscription.channel_id).await? {
                already_decided += 1;
                continue;
            }

            if fetched_game.is_none() {
                let mut full = client
                    .fetch_game(game.id)
                    .await
                    .with_context(|| format!("Failed to fetch OpenCritic game {}", game.id))?;
                full.fill_missing_from(game);
                fetched_game = Some(full);
            }
            let announcement_game = fetched_game.as_ref().expect("just ensured");

            if opencritic_announcement_exists(pool, game.id, &subscription.channel_id).await? {
                insert_opencritic_decision(
                    pool,
                    announcement_game,
                    &subscription.channel_id,
                    DecisionStatus::Announced,
                    &now,
                )
                .await?;
                continue;
            }

            let channel_id = match subscription.channel_id.parse::<u64>() {
                Ok(id) => serenity::ChannelId::new(id),
                Err(err) => {
                    error!(
                        channel = %subscription.channel_id,
                        error = %err,
                        "Invalid subscribed channel ID; skipping for this poll"
                    );
                    continue;
                }
            };

            match send_opencritic_announcement(http, channel_id, announcement_game).await {
                Ok(sent_message) => {
                    insert_opencritic_announcement(
                        pool,
                        game.id,
                        &subscription.guild_id,
                        &subscription.channel_id,
                        &sent_message.id.to_string(),
                        &now,
                    )
                    .await?;
                    insert_opencritic_decision(
                        pool,
                        announcement_game,
                        &subscription.channel_id,
                        DecisionStatus::Announced,
                        &now,
                    )
                    .await?;
                    messages_sent += 1;
                }
                Err(err) if is_channel_gone_error(&err) => {
                    warn!(
                        channel = %subscription.channel_id,
                        game_id = game.id,
                        "Discord reports channel is gone (10003); auto-unsubscribing"
                    );
                    delete_opencritic_subscription(pool, &subscription.channel_id).await?;
                    removed_channels.insert(subscription.channel_id.clone());
                }
                Err(err) => {
                    error!(
                        channel = %subscription.channel_id,
                        game_id = game.id,
                        error = %err,
                        "Failed to post OpenCritic announcement; recording decision anyway to avoid retry storms"
                    );
                    insert_opencritic_decision(
                        pool,
                        announcement_game,
                        &subscription.channel_id,
                        DecisionStatus::Announced,
                        &now,
                    )
                    .await?;
                }
            }
        }
    }

    Ok(PollOutcome {
        scanned_games: scan.games.len(),
        active_subscriptions: subscriptions.len(),
        messages_sent,
        already_decided,
        pages_requested: scan.pages_requested,
        rate_limit_remaining: scan.rate_limit_remaining,
    })
}

#[derive(Debug)]
pub struct RebaselineOutcome {
    pub newly_suppressed: usize,
}

pub async fn rebaseline_channel(
    pool: &SqlitePool,
    client: &OpenCriticClient,
    subscription: &OpenCriticSubscription,
) -> Result<RebaselineOutcome> {
    let years = scan_years();
    let scan = client
        .scan_years(&years, subscription.threshold, subscription.min_reviews)
        .await?;
    let now = Utc::now().to_rfc3339();
    let mut newly_suppressed = 0;

    for game in &scan.games {
        if opencritic_decision_exists(pool, game.id, &subscription.channel_id).await? {
            continue;
        }
        insert_opencritic_decision(
            pool,
            game,
            &subscription.channel_id,
            DecisionStatus::BaselineSuppressed,
            &now,
        )
        .await?;
        newly_suppressed += 1;
    }

    Ok(RebaselineOutcome {
        newly_suppressed,
    })
}

pub fn format_opencritic_announcement(game: &OpenCriticGame) -> String {
    let rounded_score = game.top_critic_score.round() as i64;
    let recommended = if game.percent_recommended >= 0.0 {
        format!("{:.0}%", game.percent_recommended)
    } else {
        "N/A".to_string()
    };
    let tier = if game.tier.is_empty() {
        "N/A".to_string()
    } else {
        game.tier.clone()
    };
    let release_date = game
        .pc_release_date()
        .or_else(|| game.first_release_date.as_deref().and_then(non_empty_str))
        .map(format_opencritic_date)
        .unwrap_or_else(|| "unknown".to_string());
    let url = game.url.as_deref().unwrap_or("https://opencritic.com");
    let steam_search_url = steam_search_url(&game.name);

    format!(
        "## [{}]({}) reached {} on OpenCritic\n\n- **Top Critic Average:** {:.1}\n- **Recommended:** {}\n- **Reviews:** {}\n- **Tier:** {}\n- **Release date for PC:** {}\nSearch for the game on Steam [here]({}).",
        game.name,
        url,
        rounded_score,
        game.top_critic_score,
        recommended,
        game.num_reviews,
        tier,
        release_date,
        steam_search_url,
    )
}

fn steam_search_url(game_name: &str) -> String {
    let mut encoded = String::new();
    for byte in game_name.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char);
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }

    format!("https://store.steampowered.com/search/?term={encoded}")
}

fn format_opencritic_date(value: &str) -> String {
    value
        .split('T')
        .next()
        .filter(|date| !date.is_empty())
        .unwrap_or(value)
        .to_string()
}

fn non_empty_str(value: &str) -> Option<&str> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub async fn send_opencritic_announcement(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    game: &OpenCriticGame,
) -> std::result::Result<serenity::Message, serenity::Error> {
    let mut message = serenity::CreateMessage::new()
        .content(format_opencritic_announcement(game))
        .flags(serenity::MessageFlags::SUPPRESS_EMBEDS);

    if let Some(image_url) = game.primary_image_url() {
        match serenity::CreateAttachment::url(http, &image_url).await {
            Ok(attachment) => {
                message =
                    message.add_file(attachment.description(format!("{} artwork", game.name)));
            }
            Err(err) => {
                warn!(
                    game_id = game.id,
                    image_url = %image_url,
                    error = %err,
                    "Failed to fetch OpenCritic image; sending alert without attachment"
                );
            }
        }
    }

    channel_id.send_message(http, message).await
}

pub fn spawn_opencritic_background_poll(
    http: Arc<serenity::Http>,
    pool: SqlitePool,
    client: OpenCriticClient,
    config: Arc<Config>,
    last_poll: Arc<Mutex<Option<LastPollSnapshot>>>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(config.opencritic_poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            interval.tick().await;

            info!("Running background OpenCritic poll");

            match run_opencritic_poll(&pool, &client, &http).await {
                Ok(outcome) => {
                    info!(
                        pages_requested = outcome.pages_requested,
                        scanned_games = outcome.scanned_games,
                        messages_sent = outcome.messages_sent,
                        already_decided = outcome.already_decided,
                        active_subscriptions = outcome.active_subscriptions,
                        rate_limit_remaining = outcome.rate_limit_remaining.as_deref(),
                        "Background OpenCritic poll complete"
                    );
                    *last_poll.lock().unwrap() = Some(LastPollSnapshot {
                        at: Utc::now(),
                        outcome,
                    });
                }
                Err(err) => {
                    error!(error = %err, "Background OpenCritic poll failed");
                }
            }
        }
    });
}
