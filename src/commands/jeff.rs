use std::time::Instant;

use anyhow::{Context as _, Result};
use chrono::Utc;
use tracing::{error, info, warn};

use crate::{
    context::Context,
    db::{
        delete_subscription, get_subscription, insert_subscription, ping,
        update_subscription_settings,
    },
    discord::announce::send_announcement,
    opencritic::Game,
    poller::rebaseline,
    util::humanize_duration,
};

use super::audit::CommandTimer;

const DEBUG_FIXTURES: &[(&str, &str)] = &[
    (
        "game_17732",
        include_str!("../../fixtures/opencritic/game_17732.json"),
    ),
    (
        "game_463",
        include_str!("../../fixtures/opencritic/game_463.json"),
    ),
    (
        "game_20559",
        include_str!("../../fixtures/opencritic/game_20559.json"),
    ),
];

#[poise::command(
    slash_command,
    subcommands("subscribe", "threshold", "unsubscribe", "status", "health")
)]
pub async fn jeff(_ctx: Context<'_>) -> Result<()> {
    Ok(())
}

#[poise::command(slash_command, default_member_permissions = "MANAGE_CHANNELS")]
#[tracing::instrument(
    skip_all,
    fields(
        command = "subscribe",
        user_id = %ctx.author().id,
        channel_id = %ctx.channel_id(),
        guild_id = ?ctx.guild_id().map(|g| g.get()),
        interaction_id = ctx.id(),
        score,
        min_reviews,
        min_recommended,
    ),
)]
pub async fn subscribe(
    ctx: Context<'_>,
    #[description = "Minimum top critic score required to post (e.g. 85)"]
    #[min = 1.0]
    #[max = 100.0]
    score: f64,
    #[description = "Minimum number of reviews required (e.g. 10)"]
    #[min = 1]
    min_reviews: i64,
    #[description = "Minimum percent of critics recommending the game (0-100)"]
    #[min = 0.0]
    #[max = 100.0]
    min_recommended: f64,
) -> Result<()> {
    let timer = CommandTimer::start();
    let data = ctx.data();

    if let Some(existing) = get_subscription(&data.db).await? {
        info!(
            existing_channel = %existing.channel_id,
            existing_threshold = existing.threshold,
            existing_min_reviews = existing.min_reviews,
            existing_min_percent_recommended = existing.min_percent_recommended,
            "subscribe attempted while already subscribed",
        );
        ctx.say(format!(
            "Jeff is already subscribed in <#{}> (score {:.1}, minimum reviews {}, recommended {:.0}%). \
            Use `/jeff threshold` to change values or `/jeff unsubscribe` to disable first.",
            existing.channel_id,
            existing.threshold,
            existing.min_reviews,
            existing.min_percent_recommended,
        ))
        .await?;
        timer.complete("already_subscribed");
        return Ok(());
    }

    let channel_id = ctx.channel_id();

    ctx.defer().await?;

    insert_subscription(
        &data.db,
        &channel_id.to_string(),
        score,
        min_reviews,
        min_recommended,
    )
    .await?;

    info!(
        channel_id = %channel_id,
        threshold = score,
        min_reviews,
        min_percent_recommended = min_recommended,
        "subscription created",
    );

    let subscription = get_subscription(&data.db)
        .await?
        .context("Subscription disappeared immediately after insert")?;

    let outcome = rebaseline(&data.db, &data.opencritic_client, &subscription).await?;

    info!(
        newly_suppressed = outcome.newly_suppressed,
        "baseline snapshot recorded",
    );

    ctx.say(format!(
        "OpenCritic alerts enabled in this channel.\n\
        Score: {:.1} | Minimum reviews: {} | Minimum recommended: {:.0}%\n\
        Snapshotted {} currently-qualifying games as baseline. Only new threshold crossings will be posted here.",
        score, min_reviews, min_recommended, outcome.newly_suppressed,
    ))
    .await?;

    timer.complete("ok");
    Ok(())
}

#[poise::command(slash_command, default_member_permissions = "MANAGE_CHANNELS")]
#[tracing::instrument(
    skip_all,
    fields(
        command = "threshold",
        user_id = %ctx.author().id,
        channel_id = %ctx.channel_id(),
        guild_id = ?ctx.guild_id().map(|g| g.get()),
        interaction_id = ctx.id(),
        requested_score = ?score,
        requested_min_reviews = ?min_reviews,
        requested_min_recommended = ?min_recommended,
    ),
)]
pub async fn threshold(
    ctx: Context<'_>,
    #[description = "New minimum top critic score (leave blank to keep current value)"]
    #[min = 1.0]
    #[max = 100.0]
    score: Option<f64>,
    #[description = "New minimum number of reviews (leave blank to keep current value)"]
    #[min = 1]
    min_reviews: Option<i64>,
    #[description = "New minimum percent recommended, 0-100 (leave blank to keep current value)"]
    #[min = 0.0]
    #[max = 100.0]
    min_recommended: Option<f64>,
) -> Result<()> {
    let timer = CommandTimer::start();
    let data = ctx.data();

    let Some(existing) = get_subscription(&data.db).await? else {
        ctx.say("Jeff is not subscribed. Use `/jeff subscribe` first.")
            .await?;
        timer.complete("not_subscribed");
        return Ok(());
    };

    if score.is_none() && min_reviews.is_none() && min_recommended.is_none() {
        ctx.say("Provide at least one of score, min_reviews, or min_recommended to change.")
            .await?;
        timer.complete("no_change_requested");
        return Ok(());
    }

    let new_score = score.unwrap_or(existing.threshold);
    let new_min_reviews = min_reviews.unwrap_or(existing.min_reviews);
    let new_min_recommended = min_recommended.unwrap_or(existing.min_percent_recommended);

    if (new_score - existing.threshold).abs() < f64::EPSILON
        && new_min_reviews == existing.min_reviews
        && (new_min_recommended - existing.min_percent_recommended).abs() < f64::EPSILON
    {
        ctx.say(format!(
            "Settings are already score {:.1} / {} reviews / {:.0}% recommended. No change.",
            existing.threshold, existing.min_reviews, existing.min_percent_recommended,
        ))
        .await?;
        timer.complete("no_op");
        return Ok(());
    }

    ctx.defer().await?;

    let updated =
        update_subscription_settings(&data.db, new_score, new_min_reviews, new_min_recommended)
            .await?;
    if !updated {
        warn!("subscription disappeared before update could complete");
        ctx.say("Subscription disappeared before the update could complete.")
            .await?;
        timer.complete("subscription_vanished");
        return Ok(());
    }

    info!(
        prev_threshold = existing.threshold,
        prev_min_reviews = existing.min_reviews,
        prev_min_percent_recommended = existing.min_percent_recommended,
        threshold = new_score,
        min_reviews = new_min_reviews,
        min_percent_recommended = new_min_recommended,
        "subscription updated",
    );

    let subscription = get_subscription(&data.db)
        .await?
        .context("Subscription disappeared immediately after update")?;

    let outcome = rebaseline(&data.db, &data.opencritic_client, &subscription).await?;

    info!(
        newly_suppressed = outcome.newly_suppressed,
        "baseline snapshot recorded",
    );

    ctx.say(format!(
        "Thresholds updated.\n\
        Previous: {:.1} / {} reviews / {:.0}% recommended\n\
        New: {:.1} / {} reviews / {:.0}% recommended\n\
        Snapshotted {} newly-qualifying games as baseline.",
        existing.threshold,
        existing.min_reviews,
        existing.min_percent_recommended,
        new_score,
        new_min_reviews,
        new_min_recommended,
        outcome.newly_suppressed,
    ))
    .await?;

    timer.complete("ok");
    Ok(())
}

#[poise::command(slash_command, default_member_permissions = "MANAGE_CHANNELS")]
#[tracing::instrument(
    skip_all,
    fields(
        command = "unsubscribe",
        user_id = %ctx.author().id,
        channel_id = %ctx.channel_id(),
        guild_id = ?ctx.guild_id().map(|g| g.get()),
        interaction_id = ctx.id(),
    ),
)]
pub async fn unsubscribe(ctx: Context<'_>) -> Result<()> {
    let timer = CommandTimer::start();
    let removed = delete_subscription(&ctx.data().db).await?;

    if removed {
        info!("subscription deleted");
        ctx.say("OpenCritic alerts disabled.").await?;
        timer.complete("ok");
    } else {
        ctx.say("Jeff was not subscribed.").await?;
        timer.complete("not_subscribed");
    }

    Ok(())
}

#[poise::command(slash_command)]
#[tracing::instrument(
    skip_all,
    fields(
        command = "status",
        user_id = %ctx.author().id,
        channel_id = %ctx.channel_id(),
        guild_id = ?ctx.guild_id().map(|g| g.get()),
        interaction_id = ctx.id(),
    ),
)]
pub async fn status(ctx: Context<'_>) -> Result<()> {
    let timer = CommandTimer::start();
    let subscription = get_subscription(&ctx.data().db).await?;

    let message = match &subscription {
        Some(sub) => format!(
            "Jeff is subscribed in <#{}>.\nScore: {:.1} | Minimum reviews: {} | Minimum recommended: {:.0}%",
            sub.channel_id, sub.threshold, sub.min_reviews, sub.min_percent_recommended,
        ),
        None => "Jeff is not subscribed. Use `/jeff subscribe` to enable alerts.".to_string(),
    };

    info!(subscribed = subscription.is_some(), "status reported");

    ctx.say(message).await?;
    timer.complete("ok");

    Ok(())
}

#[poise::command(slash_command, default_member_permissions = "MANAGE_CHANNELS")]
#[tracing::instrument(
    skip_all,
    fields(
        command = "debug_post",
        user_id = %ctx.author().id,
        channel_id = %ctx.channel_id(),
        guild_id = ?ctx.guild_id().map(|g| g.get()),
        interaction_id = ctx.id(),
    ),
)]
pub async fn debug_post(ctx: Context<'_>) -> Result<()> {
    let timer = CommandTimer::start();
    info!(fixtures = DEBUG_FIXTURES.len(), "debug_post started");
    ctx.defer().await?;

    let channel_id = ctx.channel_id();
    let http = ctx.serenity_context().http.clone();

    let mut posted = 0;
    let mut failures: Vec<String> = Vec::new();

    for (label, raw) in DEBUG_FIXTURES {
        let game: Game = match serde_json::from_str(raw) {
            Ok(game) => game,
            Err(err) => {
                error!(fixture = label, error = ?err, "debug_post: failed to parse fixture");
                failures.push(format!("{label}: parse failed ({err})"));
                continue;
            }
        };

        if let Err(err) = send_announcement(&http, channel_id, &game).await {
            error!(
                fixture = label,
                game_id = game.id,
                error = ?err,
                "debug_post: failed to send"
            );
            failures.push(format!("{label}: send failed ({err})"));
            continue;
        }

        posted += 1;
    }

    info!(posted, failures = failures.len(), "debug_post finished",);

    let summary = if failures.is_empty() {
        format!("Posted {posted} debug fixture(s).")
    } else {
        format!(
            "Posted {posted} debug fixture(s). Failures:\n{}",
            failures.join("\n")
        )
    };

    ctx.say(summary).await?;
    timer.complete(if failures.is_empty() { "ok" } else { "partial" });
    Ok(())
}

#[poise::command(slash_command)]
#[tracing::instrument(
    skip_all,
    fields(
        command = "health",
        user_id = %ctx.author().id,
        channel_id = %ctx.channel_id(),
        guild_id = ?ctx.guild_id().map(|g| g.get()),
        interaction_id = ctx.id(),
    ),
)]
pub async fn health(ctx: Context<'_>) -> Result<()> {
    let timer = CommandTimer::start();
    ctx.defer().await?;

    let data = ctx.data();
    let now = Utc::now();

    let uptime = now
        .signed_duration_since(data.started_at)
        .to_std()
        .unwrap_or_default();

    let db_start = Instant::now();
    let db_result = ping(&data.db).await;
    let db_elapsed = db_start.elapsed();
    let db_status = match &db_result {
        Ok(()) => format!("ok ({}ms)", db_elapsed.as_millis()),
        Err(err) => format!("error: {err}"),
    };

    let shard_latency = {
        let runners = ctx.framework().shard_manager.runners.lock().await;
        runners.values().next().and_then(|r| r.latency)
    };
    let latency_str = match shard_latency {
        Some(d) => format!("{}ms", d.as_millis()),
        None => "pending first heartbeat".to_string(),
    };

    let rate_limit = data
        .opencritic_client
        .last_rate_limit_remaining()
        .map(|remaining| remaining.to_string())
        .unwrap_or_else(|| "unknown (no API call yet)".to_string());

    let subscribed = get_subscription(&data.db).await?.is_some();

    let last_poll = data.last_poll.lock().unwrap().clone();
    let last_poll_str = match last_poll {
        Some(snap) => {
            let ago = now
                .signed_duration_since(snap.completed_at)
                .to_std()
                .unwrap_or_default();
            format!(
                "{} ago — {} alert(s) posted",
                humanize_duration(ago),
                snap.outcome.messages_sent
            )
        }
        None => "none yet".to_string(),
    };

    info!(
        uptime_ms = uptime.as_millis() as u64,
        db_ok = db_result.is_ok(),
        db_elapsed_ms = db_elapsed.as_millis() as u64,
        shard_latency_ms = shard_latency.map(|d| d.as_millis() as u64),
        rate_limit_remaining = %rate_limit,
        subscribed,
        "health snapshot",
    );

    ctx.say(format!(
        "Health check:\n\
        Uptime: {}\n\
        Database: {}\n\
        Discord gateway latency: {}\n\
        OpenCritic API quota remaining: {}\n\
        Subscribed: {}\n\
        Last successful poll: {}",
        humanize_duration(uptime),
        db_status,
        latency_str,
        rate_limit,
        if subscribed { "yes" } else { "no" },
        last_poll_str,
    ))
    .await?;

    timer.complete("ok");
    Ok(())
}
