use anyhow::{Context as _, Result};
use chrono::Utc;

use crate::{
    context::Context,
    db::{
        count_opencritic_decisions, delete_opencritic_subscription, upsert_opencritic_subscription,
    },
    poller::{PollMode, run_opencritic_poll},
};

#[poise::command(slash_command, prefix_command, rename = "opencritic_subscribe")]
pub async fn opencritic_subscribe(ctx: Context<'_>) -> Result<()> {
    let guild_id = ctx
        .guild_id()
        .context("OpenCritic subscriptions can only be configured in a server")?;
    let channel_id = ctx.channel_id();
    let created_by = ctx.author().id.to_string();
    let created_at = Utc::now().to_rfc3339();

    upsert_opencritic_subscription(
        &ctx.data().db,
        &guild_id.to_string(),
        &channel_id.to_string(),
        &created_by,
        &created_at,
    )
    .await?;

    ctx.say("OpenCritic alerts enabled in this channel.")
        .await?;

    Ok(())
}

#[poise::command(slash_command, prefix_command, rename = "opencritic_unsubscribe")]
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

#[poise::command(slash_command, prefix_command, rename = "opencritic_check")]
pub async fn check_opencritic(
    ctx: Context<'_>,
    #[description = "Post new qualifying games to subscribed channels instead of only reporting what would happen"]
    post: Option<bool>,
) -> Result<()> {
    ctx.defer().await?;

    let post = post.unwrap_or(false);
    let data = ctx.data();
    let mode = if post {
        PollMode::Live {
            http: ctx.serenity_context().http.as_ref(),
        }
    } else {
        PollMode::DryRun
    };

    let outcome =
        run_opencritic_poll(&data.db, &data.opencritic_client, &data.config, mode).await?;

    let mode_label = if post { "post" } else { "dry-run" };
    let baseline = if outcome.baseline_created {
        "created baseline; posted nothing"
    } else if outcome.baseline_completed_before_run {
        "already complete"
    } else {
        unreachable!("OpenCritic baseline is unexpectedly incomplete after poll")
    };
    let rate_limit = outcome.rate_limit_remaining.as_deref().unwrap_or("unknown");

    ctx.say(format!(
        "OpenCritic check complete ({mode_label}).\n\
        Years: {:?}\n\
        Pages requested: {}\n\
        RapidAPI requests remaining: {}\n\
        Active subscriptions: {}\n\
        Baseline: {}\n\
        Qualifying games found: {}\n\
        Baseline suppressed: {}\n\
        Existing decisions skipped: {}\n\
        Would handle games: {}\n\
        Handled games: {}\n\
        Messages sent: {}",
        outcome.years,
        outcome.pages_requested,
        rate_limit,
        outcome.active_subscriptions,
        baseline,
        outcome.qualifying_games,
        outcome.baseline_suppressed,
        outcome.skipped_existing_decision,
        outcome.would_handle_games,
        outcome.handled_games,
        outcome.messages_sent,
    ))
    .await?;

    Ok(())
}

#[poise::command(slash_command, prefix_command, rename = "opencritic_status")]
pub async fn opencritic_status(ctx: Context<'_>) -> Result<()> {
    let data = ctx.data();
    let (baseline_suppressed, handled_games, subscriptions, announcement_deliveries) =
        count_opencritic_decisions(&data.db).await?;

    ctx.say(format!(
        "OpenCritic status:\n\
        Threshold: {:.1}\n\
        Minimum reviews: {}\n\
        Scan previous year: {}\n\
        Poll interval: {} seconds\n\
        Active subscriptions: {}\n\
        Baseline suppressed decisions: {}\n\
        Handled qualifying games: {}\n\
        Announcement deliveries: {}",
        data.config.opencritic_threshold,
        data.config.opencritic_min_reviews,
        data.config.opencritic_scan_previous_year,
        data.config.opencritic_poll_interval.as_secs(),
        subscriptions,
        baseline_suppressed,
        handled_games,
        announcement_deliveries,
    ))
    .await?;

    Ok(())
}
