use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Datelike, Utc};
use poise::serenity_prelude::{self as serenity, HttpError};
use sqlx::SqlitePool;
use tracing::{Instrument, debug, error, field, info, info_span, warn};

use crate::{
    config::Config,
    db::{
        DecisionStatus, OpenCriticSubscription, delete_opencritic_subscription,
        get_opencritic_subscription, insert_opencritic_decision, opencritic_decision_exists,
    },
    opencritic::{OpenCriticClient, OpenCriticGame},
    util::truncate_for_log,
};

const DISCORD_UNKNOWN_CHANNEL_ERROR_CODE: isize = 10003;
const DISCORD_ERROR_BODY_TRUNCATE: usize = 512;

fn is_channel_gone_error(err: &serenity::Error) -> bool {
    matches!(
        err,
        serenity::Error::Http(HttpError::UnsuccessfulRequest(resp))
            if resp.error.code == DISCORD_UNKNOWN_CHANNEL_ERROR_CODE
    )
}

struct DiscordErrorParts {
    status: u16,
    code: isize,
    message: String,
    body: String,
}

fn discord_error_parts(err: &serenity::Error) -> Option<DiscordErrorParts> {
    if let serenity::Error::Http(HttpError::UnsuccessfulRequest(resp)) = err {
        Some(DiscordErrorParts {
            status: resp.status_code.as_u16(),
            code: resp.error.code,
            message: resp.error.message.clone(),
            body: format!("{:?}", resp.error),
        })
    } else {
        None
    }
}

fn scan_years() -> Vec<i32> {
    let current_year = Utc::now().year();
    vec![current_year, current_year - 1]
}

#[derive(Debug, Clone)]
pub struct PollOutcome {
    pub scanned_games: usize,
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

enum GameAction {
    Skip,
    AlreadyDecided,
    Sent,
    SendFailed,
    ChannelGone,
}

#[tracing::instrument(
    skip_all,
    fields(
        poll_id = %crate::tracing::short_id(),
        trigger = "scheduled",
    ),
    err(Debug),
)]
pub async fn run_opencritic_poll(
    pool: &SqlitePool,
    client: &OpenCriticClient,
    http: &serenity::Http,
) -> Result<PollOutcome> {
    let started = std::time::Instant::now();
    info!("poll cycle start");

    let Some(subscription) = get_opencritic_subscription(pool).await? else {
        info!(
            elapsed_ms = started.elapsed().as_millis() as u64,
            subscribed = false,
            "poll cycle complete (no subscription)",
        );
        return Ok(PollOutcome {
            scanned_games: 0,
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
    let mut announcements_failed = 0;
    let mut qualifying = 0;

    let channel_id = match subscription.channel_id.parse::<u64>() {
        Ok(id) => serenity::ChannelId::new(id),
        Err(err) => {
            warn!(
                channel = %subscription.channel_id,
                error = ?err,
                "Subscribed channel ID does not parse as u64; auto-unsubscribing"
            );
            delete_opencritic_subscription(pool).await?;
            return Ok(PollOutcome {
                scanned_games: scan.games.len(),
                messages_sent: 0,
                already_decided: 0,
                pages_requested: scan.pages_requested,
                rate_limit_remaining: scan.rate_limit_remaining,
            });
        }
    };

    for game in &scan.games {
        let span = info_span!(
            "announce_game",
            game_id = game.id,
            name = %game.name,
            score = game.top_critic_score,
            tier = %game.tier,
        );
        let action = async {
            if !game.qualifies(
                subscription.threshold,
                subscription.min_reviews,
                subscription.min_percent_recommended,
            ) {
                debug!(
                    qualifies = false,
                    percent_recommended = game.percent_recommended,
                    num_reviews = game.num_reviews,
                    "game evaluated",
                );
                return Ok::<GameAction, anyhow::Error>(GameAction::Skip);
            }
            if opencritic_decision_exists(pool, game.id).await? {
                debug!(qualifies = true, already_decided = true, "game evaluated");
                return Ok(GameAction::AlreadyDecided);
            }
            debug!(qualifies = true, already_decided = false, "game evaluated");

            let mut full = client
                .fetch_game(game.id)
                .await
                .with_context(|| format!("Failed to fetch OpenCritic game {}", game.id))?;
            full.fill_missing_from(game);

            match send_opencritic_announcement(http, channel_id, &full).await {
                Ok(message) => {
                    insert_opencritic_decision(pool, &full, DecisionStatus::Announced, &now)
                        .await?;
                    info!(
                        channel_id = %channel_id,
                        message_id = %message.id,
                        "announcement posted",
                    );
                    Ok(GameAction::Sent)
                }
                Err(err) if is_channel_gone_error(&err) => {
                    warn!(
                        channel = %subscription.channel_id,
                        error = ?err,
                        "Discord reports channel is gone (10003); auto-unsubscribing",
                    );
                    delete_opencritic_subscription(pool).await?;
                    Ok(GameAction::ChannelGone)
                }
                Err(err) => {
                    match discord_error_parts(&err) {
                        Some(parts) => error!(
                            channel = %subscription.channel_id,
                            discord_status = parts.status,
                            discord_code = parts.code,
                            discord_message = %parts.message,
                            discord_body = %truncate_for_log(&parts.body, DISCORD_ERROR_BODY_TRUNCATE),
                            error = ?err,
                            "Failed to post announcement; recording decision anyway to prevent retry storm",
                        ),
                        None => error!(
                            channel = %subscription.channel_id,
                            error = ?err,
                            "Failed to post announcement; recording decision anyway to prevent retry storm",
                        ),
                    }
                    insert_opencritic_decision(pool, &full, DecisionStatus::Announced, &now)
                        .await?;
                    Ok(GameAction::SendFailed)
                }
            }
        }
        .instrument(span)
        .await?;

        match action {
            GameAction::Skip => {}
            GameAction::AlreadyDecided => {
                qualifying += 1;
                already_decided += 1;
            }
            GameAction::Sent => {
                qualifying += 1;
                messages_sent += 1;
            }
            GameAction::SendFailed => {
                qualifying += 1;
                announcements_failed += 1;
            }
            GameAction::ChannelGone => {
                qualifying += 1;
                break;
            }
        }
    }

    let outcome = PollOutcome {
        scanned_games: scan.games.len(),
        messages_sent,
        already_decided,
        pages_requested: scan.pages_requested,
        rate_limit_remaining: scan.rate_limit_remaining,
    };

    info!(
        scanned_games = outcome.scanned_games,
        qualifying_games = qualifying,
        announcements_attempted = messages_sent + announcements_failed,
        announcements_succeeded = messages_sent,
        announcements_failed,
        already_decided = outcome.already_decided,
        pages_requested = outcome.pages_requested,
        rate_limit_remaining = outcome.rate_limit_remaining.as_deref(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "poll cycle complete",
    );

    Ok(outcome)
}

#[derive(Debug)]
pub struct RebaselineOutcome {
    pub newly_suppressed: usize,
}

#[tracing::instrument(
    skip_all,
    fields(
        threshold = subscription.threshold,
        min_reviews = subscription.min_reviews,
        min_percent_recommended = subscription.min_percent_recommended,
    ),
)]
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

pub struct RefreshThrottle {
    cooldown: Duration,
    last_press: Mutex<HashMap<i64, Instant>>,
}

impl RefreshThrottle {
    pub fn new(cooldown: Duration) -> Self {
        Self {
            cooldown,
            last_press: Mutex::new(HashMap::new()),
        }
    }

    pub fn cooldown(&self) -> Duration {
        self.cooldown
    }

    pub fn check(&self, game_id: i64) -> Result<(), Duration> {
        if self.cooldown.is_zero() {
            return Ok(());
        }
        let map = self.last_press.lock().unwrap();
        if let Some(last) = map.get(&game_id) {
            let elapsed = last.elapsed();
            if elapsed < self.cooldown {
                return Err(self.cooldown - elapsed);
            }
        }
        Ok(())
    }

    pub fn record(&self, game_id: i64) {
        if self.cooldown.is_zero() {
            return;
        }
        self.last_press
            .lock()
            .unwrap()
            .insert(game_id, Instant::now());
    }
}

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

    embed = embed
        .footer(serenity::CreateEmbedFooter::new("Last refreshed"))
        .timestamp(Utc::now());

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

#[tracing::instrument(
    skip_all,
    fields(
        custom_id = %interaction.data.custom_id,
        user_id = %interaction.user.id,
        message_id = %interaction.message.id,
        interaction_id = %interaction.id,
        game_id = field::Empty,
    ),
)]
pub async fn handle_refresh_button(
    ctx: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
    client: &OpenCriticClient,
    throttle: &RefreshThrottle,
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
            warn!(error = ?err, "Refresh: failed to parse game id from custom_id");
            return Ok(());
        }
    };

    tracing::Span::current().record("game_id", game_id);
    info!("Refresh requested");

    if let Err(remaining) = throttle.check(game_id) {
        let remaining_secs = remaining.as_secs().max(1);
        info!(
            remaining_secs,
            cooldown_secs = throttle.cooldown().as_secs(),
            "Refresh denied (cooldown)",
        );
        interaction
            .create_response(
                &ctx.http,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .content(format!(
                            "Refresh on cooldown, try again in {remaining_secs}s.",
                        ))
                        .ephemeral(true),
                ),
            )
            .await
            .context("Failed to send refresh cooldown response")?;
        return Ok(());
    }

    interaction
        .create_response(&ctx.http, serenity::CreateInteractionResponse::Acknowledge)
        .await
        .context("Failed to acknowledge refresh interaction")?;

    let game = match client.fetch_game(game_id).await {
        Ok(game) => game,
        Err(err) => {
            error!(error = ?err, "Refresh: failed to fetch game");
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

    throttle.record(game_id);
    info!("Refresh completed");

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
            debug!("poll cycle tick");

            let pool = pool.clone();
            let client = client.clone();
            let http = Arc::clone(&http);

            let join =
                tokio::spawn(async move { run_opencritic_poll(&pool, &client, &http).await });

            match join.await {
                Ok(Ok(outcome)) => {
                    *last_poll.lock().unwrap() = Some(LastPollSnapshot {
                        at: Utc::now(),
                        outcome,
                    });
                }
                Ok(Err(_)) => {
                    // Error already logged inside run_opencritic_poll via err(Debug).
                }
                Err(join_err) => {
                    error!(
                        join_error = ?join_err,
                        "Poll cycle task panicked or was cancelled; loop continues",
                    );
                }
            }
        }
    });
}
