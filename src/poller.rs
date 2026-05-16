use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Datelike, Utc};
use poise::serenity_prelude::{self as serenity, HttpError};
use sqlx::SqlitePool;
use tracing::{error, info, warn};

use crate::{
    config::Config,
    db::{
        DecisionStatus, OpenCriticSubscription, delete_opencritic_subscription,
        get_opencritic_subscription, insert_opencritic_decision, opencritic_decision_exists,
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

fn scan_years() -> Vec<i32> {
    let current_year = Utc::now().year();
    vec![current_year, current_year - 1]
}

#[derive(Debug, Clone)]
pub struct PollOutcome {
    pub scanned_games: usize,
    pub subscribed: bool,
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
    let Some(subscription) = get_opencritic_subscription(pool).await? else {
        return Ok(PollOutcome {
            scanned_games: 0,
            subscribed: false,
            messages_sent: 0,
            already_decided: 0,
            pages_requested: 0,
            rate_limit_remaining: client.last_rate_limit_remaining(),
        });
    };

    let years = scan_years();
    let scan = client
        .scan_years(
            &years,
            subscription.threshold,
            subscription.min_reviews,
            subscription.min_percent_recommended,
        )
        .await?;

    let now = Utc::now().to_rfc3339();
    let mut messages_sent = 0;
    let mut already_decided = 0;

    let channel_id = match subscription.channel_id.parse::<u64>() {
        Ok(id) => serenity::ChannelId::new(id),
        Err(err) => {
            warn!(
                channel = %subscription.channel_id,
                error = %err,
                "Subscribed channel ID does not parse as u64; auto-unsubscribing"
            );
            delete_opencritic_subscription(pool).await?;
            return Ok(PollOutcome {
                scanned_games: scan.games.len(),
                subscribed: false,
                messages_sent: 0,
                already_decided: 0,
                pages_requested: scan.pages_requested,
                rate_limit_remaining: scan.rate_limit_remaining,
            });
        }
    };

    for game in &scan.games {
        if !game.qualifies(
            subscription.threshold,
            subscription.min_reviews,
            subscription.min_percent_recommended,
        ) {
            continue;
        }
        if opencritic_decision_exists(pool, game.id).await? {
            already_decided += 1;
            continue;
        }

        let mut full = client
            .fetch_game(game.id)
            .await
            .with_context(|| format!("Failed to fetch OpenCritic game {}", game.id))?;
        full.fill_missing_from(game);

        match send_opencritic_announcement(http, channel_id, &full).await {
            Ok(_) => {
                insert_opencritic_decision(pool, &full, DecisionStatus::Announced, &now).await?;
                messages_sent += 1;
            }
            Err(err) if is_channel_gone_error(&err) => {
                warn!(
                    channel = %subscription.channel_id,
                    game_id = game.id,
                    "Discord reports channel is gone (10003); auto-unsubscribing"
                );
                delete_opencritic_subscription(pool).await?;
                break;
            }
            Err(err) => {
                error!(
                    channel = %subscription.channel_id,
                    game_id = game.id,
                    error = %err,
                    "Failed to post OpenCritic announcement; recording decision anyway to avoid retry storms"
                );
                insert_opencritic_decision(pool, &full, DecisionStatus::Announced, &now).await?;
            }
        }
    }

    Ok(PollOutcome {
        scanned_games: scan.games.len(),
        subscribed: true,
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

pub async fn rebaseline(
    pool: &SqlitePool,
    client: &OpenCriticClient,
    subscription: &OpenCriticSubscription,
) -> Result<RebaselineOutcome> {
    let years = scan_years();
    let scan = client
        .scan_years(
            &years,
            subscription.threshold,
            subscription.min_reviews,
            subscription.min_percent_recommended,
        )
        .await?;
    let now = Utc::now().to_rfc3339();
    let mut newly_suppressed = 0;

    for game in &scan.games {
        if opencritic_decision_exists(pool, game.id).await? {
            continue;
        }
        insert_opencritic_decision(pool, game, DecisionStatus::BaselineSuppressed, &now).await?;
        newly_suppressed += 1;
    }

    Ok(RebaselineOutcome { newly_suppressed })
}

const DESCRIPTION_MAX_LEN: usize = 400;
pub const REFRESH_BUTTON_PREFIX: &str = "refresh_game:";

pub fn build_opencritic_embed(game: &OpenCriticGame) -> serenity::CreateEmbed {
    let url = game
        .url
        .as_deref()
        .and_then(non_empty_str)
        .unwrap_or("https://opencritic.com")
        .to_string();

    let mut embed = serenity::CreateEmbed::new()
        .title(&game.name)
        .url(url)
        .author(serenity::CreateEmbedAuthor::new("OpenCritic").url("https://opencritic.com"));

    if let Some(color) = embed_color_for_tier(&game.tier) {
        embed = embed.colour(color);
    }

    if let Some(description) = game.description.as_deref().and_then(non_empty_str) {
        embed = embed.description(truncate_description(description));
    }

    if let Some(image_url) = game.primary_image_url() {
        embed = embed.image(image_url);
    }

    if let Some(box_art) = game.box_art_url() {
        embed = embed.thumbnail(box_art);
    }

    let tier_value = if game.tier.is_empty() {
        "N/A".to_string()
    } else {
        game.tier.clone()
    };

    embed = embed
        .field(
            "Critic Average",
            format!("{:.1}", game.top_critic_score),
            true,
        )
        .field(
            "Recommended",
            if game.percent_recommended >= 0.0 {
                format!("{:.0}%", game.percent_recommended)
            } else {
                "N/A".to_string()
            },
            true,
        )
        .field("Reviews", game.num_reviews.to_string(), true)
        .field("Tier", tier_value, true);

    let release_date = game
        .pc_release_date()
        .or_else(|| game.first_release_date.as_deref().and_then(non_empty_str))
        .map(format_opencritic_date)
        .unwrap_or_else(|| "unknown".to_string());
    embed = embed.field("PC Release", release_date, true);

    let genres = game.genre_names();
    if !genres.is_empty() {
        embed = embed.field("Genres", genres.join(", "), true);
    }

    let developers = game.developers();
    if !developers.is_empty() {
        embed = embed.field("Developer", developers.join(", "), true);
    }

    let publishers = game.publishers();
    if !publishers.is_empty() {
        embed = embed.field("Publisher", publishers.join(", "), true);
    }

    embed = embed.field(
        "Steam",
        format!("[Search]({})", steam_search_url(&game.name)),
        true,
    );

    if let Some(latest) = game
        .latest_review_date
        .as_deref()
        .and_then(non_empty_str)
        .map(format_opencritic_date)
    {
        embed = embed.footer(serenity::CreateEmbedFooter::new(format!(
            "Latest review · {latest}"
        )));
    }

    embed = embed.timestamp(Utc::now());

    embed
}

pub fn build_refresh_action_row(game_id: i64) -> serenity::CreateActionRow {
    serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{REFRESH_BUTTON_PREFIX}{game_id}"))
            .label("Refresh")
            .style(serenity::ButtonStyle::Secondary),
    ])
}

fn embed_color_for_tier(tier: &str) -> Option<serenity::Colour> {
    match tier {
        "Mighty" => Some(serenity::Colour::new(0xE6B800)),
        "Strong" => Some(serenity::Colour::new(0x2ECC71)),
        "Fair" => Some(serenity::Colour::new(0x3498DB)),
        "Weak" => Some(serenity::Colour::new(0x95A5A6)),
        _ => None,
    }
}

fn truncate_description(value: &str) -> String {
    if value.chars().count() <= DESCRIPTION_MAX_LEN {
        return value.to_string();
    }

    let truncated: String = value.chars().take(DESCRIPTION_MAX_LEN).collect();
    let cut_at = truncated
        .rfind(char::is_whitespace)
        .unwrap_or(truncated.len());
    let mut result = truncated[..cut_at].trim_end().to_string();
    result.push('…');
    result
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
    let message = serenity::CreateMessage::new()
        .embed(build_opencritic_embed(game))
        .components(vec![build_refresh_action_row(game.id)]);
    channel_id.send_message(http, message).await
}

pub async fn handle_refresh_button(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    client: &OpenCriticClient,
) -> Result<()> {
    let Some(game_id_str) = interaction
        .data
        .custom_id
        .strip_prefix(REFRESH_BUTTON_PREFIX)
    else {
        return Ok(());
    };

    let game_id: i64 = match game_id_str.parse() {
        Ok(id) => id,
        Err(err) => {
            warn!(
                custom_id = %interaction.data.custom_id,
                error = %err,
                "Refresh: failed to parse game id from custom_id"
            );
            return Ok(());
        }
    };

    info!(
        game_id,
        message_id = %interaction.message.id,
        user_id = %interaction.user.id,
        "Refresh requested"
    );

    interaction
        .create_response(&ctx.http, serenity::CreateInteractionResponse::Acknowledge)
        .await
        .context("Failed to acknowledge refresh interaction")?;

    let game = match client.fetch_game(game_id).await {
        Ok(game) => game,
        Err(err) => {
            error!(game_id, error = %err, "Refresh: failed to fetch game");
            interaction
                .create_followup(
                    &ctx.http,
                    serenity::CreateInteractionResponseFollowup::new()
                        .content(format!("Failed to refresh game {game_id}: {err}"))
                        .ephemeral(true),
                )
                .await
                .context("Failed to send refresh error followup")?;
            return Ok(());
        }
    };

    interaction
        .channel_id
        .edit_message(
            &ctx.http,
            interaction.message.id,
            serenity::EditMessage::new()
                .embed(build_opencritic_embed(&game))
                .components(vec![build_refresh_action_row(game.id)]),
        )
        .await
        .context("Failed to edit refreshed announcement")?;

    info!(game_id, "Refresh completed");

    Ok(())
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
                        subscribed = outcome.subscribed,
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
