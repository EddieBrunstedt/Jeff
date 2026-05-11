use std::time::Instant;

use anyhow::{Context as _, Result};
use chrono::Utc;

use crate::{
    context::Context,
    db::{
        delete_opencritic_subscription, get_opencritic_subscription,
        insert_opencritic_subscription, ping, update_opencritic_subscription_settings,
    },
    poller::rebaseline,
    util::humanize_duration,
};

const DEFAULT_MIN_REVIEWS: i64 = 10;

#[poise::command(
    slash_command,
    subcommands("subscribe", "threshold", "unsubscribe", "status", "health")
)]
pub async fn jeff(_ctx: Context<'_>) -> Result<()> {
    Ok(())
}

#[poise::command(slash_command, default_member_permissions = "MANAGE_CHANNELS")]
pub async fn subscribe(
    ctx: Context<'_>,
    #[description = "Minimum top critic score required to post (e.g. 85)"]
    #[min = 1.0]
    #[max = 100.0]
    score: f64,
    #[description = "Minimum number of reviews required (default 10)"]
    #[min = 1]
    min_reviews: Option<i64>,
) -> Result<()> {
    let data = ctx.data();

    if let Some(existing) = get_opencritic_subscription(&data.db).await? {
        ctx.say(format!(
            "Jeff is already subscribed in <#{}> (threshold {:.1}, minimum reviews {}). \
            Use `/jeff threshold` to change the threshold or `/jeff unsubscribe` to disable first.",
            existing.channel_id, existing.threshold, existing.min_reviews
        ))
        .await?;
        return Ok(());
    }

    let channel_id = ctx.channel_id();
    let min_reviews = min_reviews.unwrap_or(DEFAULT_MIN_REVIEWS);

    ctx.defer().await?;

    insert_opencritic_subscription(&data.db, &channel_id.to_string(), score, min_reviews).await?;

    let subscription = get_opencritic_subscription(&data.db)
        .await?
        .context("Subscription disappeared immediately after insert")?;

    let outcome = rebaseline(&data.db, &data.opencritic_client, &subscription).await?;

    ctx.say(format!(
        "OpenCritic alerts enabled in this channel.\n\
        Threshold: {:.1} | Minimum reviews: {}\n\
        Snapshotted {} currently-qualifying games as baseline. Only new threshold crossings will be posted here.",
        score, min_reviews, outcome.newly_suppressed,
    ))
    .await?;

    Ok(())
}

#[poise::command(slash_command, default_member_permissions = "MANAGE_CHANNELS")]
pub async fn threshold(
    ctx: Context<'_>,
    #[description = "New minimum top critic score required to post (e.g. 85)"]
    #[min = 1.0]
    #[max = 100.0]
    score: f64,
    #[description = "New minimum number of reviews required (leave blank to keep current value)"]
    #[min = 1]
    min_reviews: Option<i64>,
) -> Result<()> {
    let data = ctx.data();

    let Some(existing) = get_opencritic_subscription(&data.db).await? else {
        ctx.say("Jeff is not subscribed. Use `/jeff subscribe` first.")
            .await?;
        return Ok(());
    };

    let new_min_reviews = min_reviews.unwrap_or(existing.min_reviews);

    if (score - existing.threshold).abs() < f64::EPSILON
        && new_min_reviews == existing.min_reviews
    {
        ctx.say(format!(
            "Threshold is already {:.1} with {} minimum reviews. No change.",
            existing.threshold, existing.min_reviews
        ))
        .await?;
        return Ok(());
    }

    ctx.defer().await?;

    let updated =
        update_opencritic_subscription_settings(&data.db, score, new_min_reviews).await?;
    if !updated {
        ctx.say("Subscription disappeared before the update could complete.")
            .await?;
        return Ok(());
    }

    let subscription = get_opencritic_subscription(&data.db)
        .await?
        .context("Subscription disappeared immediately after update")?;

    let outcome = rebaseline(&data.db, &data.opencritic_client, &subscription).await?;

    ctx.say(format!(
        "Threshold updated.\n\
        Previous: {:.1} / {} reviews — New: {:.1} / {} reviews\n\
        Snapshotted {} newly-qualifying games as baseline.",
        existing.threshold, existing.min_reviews, score, new_min_reviews, outcome.newly_suppressed,
    ))
    .await?;

    Ok(())
}

#[poise::command(slash_command, default_member_permissions = "MANAGE_CHANNELS")]
pub async fn unsubscribe(ctx: Context<'_>) -> Result<()> {
    let removed = delete_opencritic_subscription(&ctx.data().db).await?;

    if removed {
        ctx.say("OpenCritic alerts disabled.").await?;
    } else {
        ctx.say("Jeff was not subscribed.").await?;
    }

    Ok(())
}

#[poise::command(slash_command)]
pub async fn status(ctx: Context<'_>) -> Result<()> {
    let subscription = get_opencritic_subscription(&ctx.data().db).await?;

    let message = match subscription {
        Some(sub) => format!(
            "Jeff is subscribed in <#{}>.\nThreshold: {:.1} | Minimum reviews: {}",
            sub.channel_id, sub.threshold, sub.min_reviews
        ),
        None => "Jeff is not subscribed. Use `/jeff subscribe` to enable alerts.".to_string(),
    };

    ctx.say(message).await?;

    Ok(())
}

#[poise::command(slash_command)]
pub async fn health(ctx: Context<'_>) -> Result<()> {
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
    let db_status = match db_result {
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
        .unwrap_or_else(|| "unknown (no API call yet)".to_string());

    let subscribed = get_opencritic_subscription(&data.db).await?.is_some();

    let last_poll = data.last_poll.lock().unwrap().clone();
    let last_poll_str = match last_poll {
        Some(snap) => {
            let ago = now
                .signed_duration_since(snap.at)
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

    Ok(())
}
