use anyhow::{Context as _, Result};
use chrono::Utc;

use crate::{
    context::Context,
    db::{
        delete_opencritic_subscription, get_opencritic_subscription,
        update_opencritic_subscription_settings, upsert_opencritic_subscription,
    },
    poller::rebaseline_channel,
};

const DEFAULT_MIN_REVIEWS: i64 = 10;

#[poise::command(
    slash_command,
    rename = "opencritic_subscribe",
    default_member_permissions = "MANAGE_CHANNELS"
)]
pub async fn opencritic_subscribe(
    ctx: Context<'_>,
    #[description = "Minimum top critic score required to post (e.g. 85)"]
    #[min = 1.0]
    #[max = 100.0]
    score: f64,
    #[description = "Minimum number of reviews required (default 10)"]
    #[min = 1]
    min_reviews: Option<i64>,
) -> Result<()> {
    let guild_id = ctx
        .guild_id()
        .context("OpenCritic subscriptions can only be configured in a server")?;
    let channel_id = ctx.channel_id();
    let created_by = ctx.author().id.to_string();
    let created_at = Utc::now().to_rfc3339();
    let data = ctx.data();
    let min_reviews = min_reviews.unwrap_or(DEFAULT_MIN_REVIEWS);

    ctx.defer().await?;

    upsert_opencritic_subscription(
        &data.db,
        &guild_id.to_string(),
        &channel_id.to_string(),
        score,
        min_reviews,
        &created_by,
        &created_at,
    )
    .await?;

    let subscription = get_opencritic_subscription(&data.db, &channel_id.to_string())
        .await?
        .context("Subscription disappeared immediately after upsert")?;

    let outcome = rebaseline_channel(&data.db, &data.opencritic_client, &subscription).await?;

    ctx.say(format!(
        "OpenCritic alerts enabled in this channel.\n\
        Threshold: {:.1} | Minimum reviews: {}\n\
        Snapshotted {} currently-qualifying games as baseline. Only new threshold crossings will be posted here.",
        score, min_reviews, outcome.newly_suppressed,
    ))
    .await?;

    Ok(())
}

#[poise::command(
    slash_command,
    rename = "opencritic_set_threshold",
    default_member_permissions = "MANAGE_CHANNELS"
)]
pub async fn opencritic_set_threshold(
    ctx: Context<'_>,
    #[description = "New minimum top critic score required to post (e.g. 85)"]
    #[min = 1.0]
    #[max = 100.0]
    score: f64,
    #[description = "New minimum number of reviews required (leave blank to keep current value)"]
    #[min = 1]
    min_reviews: Option<i64>,
) -> Result<()> {
    let channel_id = ctx.channel_id();
    let data = ctx.data();

    let Some(existing) = get_opencritic_subscription(&data.db, &channel_id.to_string()).await?
    else {
        ctx.say(
            "OpenCritic alerts are not enabled in this channel. Use /opencritic_subscribe first.",
        )
        .await?;
        return Ok(());
    };

    let new_min_reviews = min_reviews.unwrap_or(existing.min_reviews);

    if (score - existing.threshold).abs() < f64::EPSILON
        && new_min_reviews == existing.min_reviews
    {
        ctx.say(format!(
            "OpenCritic threshold for this channel is already {:.1} with {} minimum reviews. No change.",
            existing.threshold, existing.min_reviews
        ))
        .await?;
        return Ok(());
    }

    ctx.defer().await?;

    let updated = update_opencritic_subscription_settings(
        &data.db,
        &channel_id.to_string(),
        score,
        new_min_reviews,
    )
    .await?;
    if !updated {
        ctx.say("Subscription disappeared before the update could complete.")
            .await?;
        return Ok(());
    }

    let subscription = get_opencritic_subscription(&data.db, &channel_id.to_string())
        .await?
        .context("Subscription disappeared immediately after update")?;

    let outcome = rebaseline_channel(&data.db, &data.opencritic_client, &subscription).await?;

    ctx.say(format!(
        "OpenCritic threshold for this channel updated.\n\
        Previous: {:.1} / {} reviews — New: {:.1} / {} reviews\n\
        Snapshotted {} newly-qualifying games as baseline.",
        existing.threshold, existing.min_reviews, score, new_min_reviews, outcome.newly_suppressed,
    ))
    .await?;

    Ok(())
}

#[poise::command(
    slash_command,
    rename = "opencritic_unsubscribe",
    default_member_permissions = "MANAGE_CHANNELS"
)]
pub async fn opencritic_unsubscribe(ctx: Context<'_>) -> Result<()> {
    let removed =
        delete_opencritic_subscription(&ctx.data().db, &ctx.channel_id().to_string()).await?;

    if removed {
        ctx.say("OpenCritic alerts disabled in this channel.")
            .await?;
    } else {
        ctx.say("OpenCritic alerts were not enabled in this channel.")
            .await?;
    }

    Ok(())
}

#[poise::command(slash_command, rename = "opencritic_status")]
pub async fn opencritic_status(ctx: Context<'_>) -> Result<()> {
    let data = ctx.data();
    let channel_id = ctx.channel_id().to_string();

    let subscription = get_opencritic_subscription(&data.db, &channel_id).await?;

    let message = match subscription {
        Some(sub) => format!(
            "OpenCritic alerts are enabled in this channel.\n\
            Threshold: {:.1} | Minimum reviews: {}",
            sub.threshold, sub.min_reviews
        ),
        None => "OpenCritic alerts are not enabled in this channel. Use /opencritic_subscribe to enable them.".to_string(),
    };

    ctx.say(message).await?;

    Ok(())
}
