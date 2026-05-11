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
        DecisionStatus, delete_opencritic_subscription, insert_opencritic_announcement,
        insert_opencritic_decision, is_opencritic_baseline_complete, list_opencritic_subscriptions,
        mark_opencritic_baseline_complete, opencritic_announcement_exists,
        opencritic_decision_exists,
    },
    opencritic::{OpenCriticClient, OpenCriticGame},
};

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
    pub baseline_completed_before_run: bool,
    pub baseline_created: bool,
    pub qualifying_games: usize,
    pub baseline_suppressed: usize,
    pub handled_games: usize,
    pub messages_sent: usize,
    pub would_handle_games: usize,
    pub skipped_existing_decision: usize,
    pub active_subscriptions: usize,
    pub pages_requested: usize,
    pub years: Vec<i32>,
    pub rate_limit_remaining: Option<String>,
}

pub enum PollMode<'a> {
    DryRun,
    Live { http: &'a serenity::Http },
}

#[derive(Debug, Clone)]
pub struct LastPollSnapshot {
    pub at: DateTime<Utc>,
    pub outcome: PollOutcome,
}

pub async fn run_opencritic_poll(
    pool: &SqlitePool,
    client: &OpenCriticClient,
    config: &Config,
    mode: PollMode<'_>,
) -> Result<PollOutcome> {
    let years = scan_years(config);
    let scan = client
        .scan_years(
            &years,
            config.opencritic_threshold,
            config.opencritic_min_reviews,
        )
        .await?;

    let baseline_completed_before_run = is_opencritic_baseline_complete(pool).await?;
    let subscriptions = list_opencritic_subscriptions(pool).await?;
    let now = Utc::now().to_rfc3339();

    if !baseline_completed_before_run {
        let mut baseline_suppressed = 0;

        for game in &scan.games {
            insert_opencritic_decision(pool, game, DecisionStatus::BaselineSuppressed, &now)
                .await?;
            baseline_suppressed += 1;
        }

        mark_opencritic_baseline_complete(pool, &now).await?;

        return Ok(PollOutcome {
            baseline_completed_before_run,
            baseline_created: true,
            qualifying_games: scan.games.len(),
            baseline_suppressed,
            handled_games: 0,
            messages_sent: 0,
            would_handle_games: 0,
            skipped_existing_decision: 0,
            active_subscriptions: subscriptions.len(),
            pages_requested: scan.pages_requested,
            years: scan.years,
            rate_limit_remaining: scan.rate_limit_remaining,
        });
    }

    let mut handled_games = 0;
    let mut messages_sent = 0;
    let mut would_handle_games = 0;
    let mut skipped_existing_decision = 0;
    let mut removed_channels: HashSet<String> = HashSet::new();

    for game in &scan.games {
        if opencritic_decision_exists(pool, game.id).await? {
            skipped_existing_decision += 1;
            continue;
        }

        match mode {
            PollMode::DryRun => {
                would_handle_games += 1;
            }
            PollMode::Live { http } => {
                if subscriptions.is_empty() {
                    would_handle_games += 1;
                    continue;
                }

                let mut announcement_game = client
                    .fetch_game(game.id)
                    .await
                    .with_context(|| format!("Failed to fetch OpenCritic game {}", game.id))?;
                announcement_game.fill_missing_from(game);
                for subscription in &subscriptions {
                    if removed_channels.contains(&subscription.channel_id) {
                        continue;
                    }
                    if opencritic_announcement_exists(pool, game.id, &subscription.channel_id)
                        .await?
                    {
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

                    match send_opencritic_announcement(http, channel_id, &announcement_game).await {
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
                                "Failed to post OpenCritic announcement; continuing"
                            );
                        }
                    }
                }

                insert_opencritic_decision(
                    pool,
                    &announcement_game,
                    DecisionStatus::Announced,
                    &now,
                )
                .await?;
                handled_games += 1;
            }
        }
    }

    Ok(PollOutcome {
        baseline_completed_before_run,
        baseline_created: false,
        qualifying_games: scan.games.len(),
        baseline_suppressed: 0,
        handled_games,
        messages_sent,
        would_handle_games,
        skipped_existing_decision,
        active_subscriptions: subscriptions.len(),
        pages_requested: scan.pages_requested,
        years: scan.years,
        rate_limit_remaining: scan.rate_limit_remaining,
    })
}

#[derive(Debug)]
pub struct RebaselineOutcome {
    pub newly_suppressed: usize,
    pub already_decided: usize,
    pub pages_requested: usize,
    pub rate_limit_remaining: Option<String>,
}

pub async fn rebaseline_for_first_subscriber(
    pool: &SqlitePool,
    client: &OpenCriticClient,
    config: &Config,
) -> Result<RebaselineOutcome> {
    let years = scan_years(config);
    let scan = client
        .scan_years(
            &years,
            config.opencritic_threshold,
            config.opencritic_min_reviews,
        )
        .await?;
    let now = Utc::now().to_rfc3339();
    let mut newly_suppressed = 0;
    let mut already_decided = 0;

    for game in &scan.games {
        if opencritic_decision_exists(pool, game.id).await? {
            already_decided += 1;
            continue;
        }
        insert_opencritic_decision(pool, game, DecisionStatus::BaselineSuppressed, &now).await?;
        newly_suppressed += 1;
    }

    Ok(RebaselineOutcome {
        newly_suppressed,
        already_decided,
        pages_requested: scan.pages_requested,
        rate_limit_remaining: scan.rate_limit_remaining,
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

            match run_opencritic_poll(&pool, &client, &config, PollMode::Live { http: &http }).await
            {
                Ok(outcome) => {
                    info!(
                        pages_requested = outcome.pages_requested,
                        qualifying_games = outcome.qualifying_games,
                        handled_games = outcome.handled_games,
                        messages_sent = outcome.messages_sent,
                        active_subscriptions = outcome.active_subscriptions,
                        baseline_created = outcome.baseline_created,
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

fn scan_years(config: &Config) -> Vec<i32> {
    let current_year = Utc::now().year();

    if config.opencritic_scan_previous_year {
        vec![current_year, current_year - 1]
    } else {
        vec![current_year]
    }
}
