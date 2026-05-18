use std::sync::{Arc, Mutex};

use anyhow::{Context as _, Result};
use chrono::{DateTime, Datelike, Utc};
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;
use tracing::{Instrument, debug, error, info, info_span, warn};

use crate::{
    config::Config,
    db::{
        DecisionStatus, Subscription, decision_exists, delete_subscription, get_subscription,
        insert_decision,
    },
    discord::{
        announce::send_announcement,
        error::{error_parts, is_channel_gone},
    },
    opencritic::Client,
};

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
    pub rate_limit_remaining: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct LastPollSnapshot {
    pub completed_at: DateTime<Utc>,
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
pub async fn run_poll_cycle(
    pool: &SqlitePool,
    client: &Client,
    http: &serenity::Http,
) -> Result<PollOutcome> {
    let started = std::time::Instant::now();
    info!("poll cycle start");

    let Some(subscription) = get_subscription(pool).await? else {
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

    let now = Utc::now();
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
            delete_subscription(pool).await?;
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
            game_name = %game.name,
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
            if decision_exists(pool, game.id).await? {
                debug!(qualifies = true, already_decided = true, "game evaluated");
                return Ok(GameAction::AlreadyDecided);
            }
            debug!(qualifies = true, already_decided = false, "game evaluated");

            let mut full = client
                .fetch_game(game.id)
                .await
                .with_context(|| format!("Failed to fetch OpenCritic game {}", game.id))?;
            full.fill_missing_from(game);

            match send_announcement(http, channel_id, &full).await {
                Ok(message) => {
                    insert_decision(pool, &full, DecisionStatus::Announced, now).await?;
                    info!(
                        channel_id = %channel_id,
                        message_id = %message.id,
                        "announcement posted",
                    );
                    Ok(GameAction::Sent)
                }
                Err(err) if is_channel_gone(&err) => {
                    warn!(
                        channel = %subscription.channel_id,
                        error = ?err,
                        "Discord reports channel is gone (10003); auto-unsubscribing",
                    );
                    delete_subscription(pool).await?;
                    Ok(GameAction::ChannelGone)
                }
                Err(err) => {
                    match error_parts(&err) {
                        Some(parts) => error!(
                            channel = %subscription.channel_id,
                            discord_status = parts.status,
                            discord_code = parts.code,
                            discord_message = %parts.message,
                            discord_body = %parts.body_truncated(),
                            error = ?err,
                            "Failed to post announcement; recording decision anyway to prevent retry storm",
                        ),
                        None => error!(
                            channel = %subscription.channel_id,
                            error = ?err,
                            "Failed to post announcement; recording decision anyway to prevent retry storm",
                        ),
                    }
                    insert_decision(pool, &full, DecisionStatus::Announced, now).await?;
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
        rate_limit_remaining = outcome.rate_limit_remaining,
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
    client: &Client,
    subscription: &Subscription,
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
    let now = Utc::now();
    let mut newly_suppressed = 0;

    for game in &scan.games {
        if decision_exists(pool, game.id).await? {
            continue;
        }
        insert_decision(pool, game, DecisionStatus::BaselineSuppressed, now).await?;
        newly_suppressed += 1;
    }

    Ok(RebaselineOutcome { newly_suppressed })
}

pub fn spawn_background_poller(
    http: Arc<serenity::Http>,
    pool: SqlitePool,
    client: Client,
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

            let join = tokio::spawn(async move { run_poll_cycle(&pool, &client, &http).await });

            match join.await {
                Ok(Ok(outcome)) => {
                    *last_poll.lock().unwrap() = Some(LastPollSnapshot {
                        completed_at: Utc::now(),
                        outcome,
                    });
                }
                Ok(Err(_)) => {
                    // Error already logged inside run_poll_cycle via err(Debug).
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
