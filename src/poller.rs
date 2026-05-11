use std::{collections::HashSet, sync::Arc};

use anyhow::Result;
use chrono::{Datelike, Utc};
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

const DISCORD_UNKNOWN_CHANNEL: isize = 10003;

fn is_channel_gone_error(err: &serenity::Error) -> bool {
    matches!(
        err,
        serenity::Error::Http(HttpError::UnsuccessfulRequest(resp))
            if resp.error.code == DISCORD_UNKNOWN_CHANNEL
    )
}

#[derive(Debug)]
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

                    let message = format_opencritic_announcement(game);
                    match channel_id.say(http, message).await {
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

                insert_opencritic_decision(pool, game, DecisionStatus::Announced, &now).await?;
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
    let release_date = game.first_release_date.as_deref().unwrap_or("unknown");
    let url = game.url.as_deref().unwrap_or("https://opencritic.com");

    format!(
        "{} hit {} OpenCritic\n\nTop Critic Average: {:.1}\nRecommended: {}\nReviews: {}\nTier: {}\nRelease date: {}\n\n{}",
        game.name,
        rounded_score,
        game.top_critic_score,
        recommended,
        game.num_reviews,
        tier,
        release_date,
        url
    )
}

pub fn spawn_opencritic_background_poll(
    http: Arc<serenity::Http>,
    pool: SqlitePool,
    client: OpenCriticClient,
    config: Arc<Config>,
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
