use anyhow::{Context as _, Result};
use chrono::Utc;
use poise::{CreateReply, serenity_prelude as serenity};

use crate::{
    context::Context,
    db::{
        count_opencritic_decisions, delete_opencritic_subscription,
        is_opencritic_baseline_complete, list_baseline_suppressed_opencritic,
        list_opencritic_subscriptions, upsert_opencritic_subscription,
    },
    poller::{PollMode, rebaseline_for_first_subscriber, run_opencritic_poll},
    util::humanize_duration,
};

#[poise::command(slash_command, rename = "opencritic_subscribe")]
pub async fn opencritic_subscribe(ctx: Context<'_>) -> Result<()> {
    let guild_id = ctx
        .guild_id()
        .context("OpenCritic subscriptions can only be configured in a server")?;
    let channel_id = ctx.channel_id();
    let created_by = ctx.author().id.to_string();
    let created_at = Utc::now().to_rfc3339();
    let data = ctx.data();

    let existing = list_opencritic_subscriptions(&data.db).await?;
    let baseline_complete = is_opencritic_baseline_complete(&data.db).await?;
    let is_first_subscriber = existing.is_empty();
    let needs_rebaseline = is_first_subscriber && baseline_complete;

    if needs_rebaseline {
        ctx.defer().await?;
    }

    let rebaseline = if needs_rebaseline {
        Some(
            rebaseline_for_first_subscriber(&data.db, &data.opencritic_client, &data.config)
                .await?,
        )
    } else {
        None
    };

    upsert_opencritic_subscription(
        &data.db,
        &guild_id.to_string(),
        &channel_id.to_string(),
        &created_by,
        &created_at,
    )
    .await?;

    let reply = match (is_first_subscriber, baseline_complete) {
        (false, _) => "OpenCritic alerts enabled in this channel.".to_string(),
        (true, false) => "OpenCritic alerts enabled. The initial baseline poll has not run yet; \
            once it does, currently-qualifying games will be suppressed and only new threshold \
            crossings will be posted here."
            .to_string(),
        (true, true) => {
            let outcome =
                rebaseline.expect("rebaseline runs when first subscriber + baseline complete");
            let rate_limit = outcome.rate_limit_remaining.as_deref().unwrap_or("unknown");
            format!(
                "OpenCritic alerts enabled. Snapshotted {} currently-qualifying games as \
                baseline ({} already in the ledger). Only new threshold crossings will be \
                posted here.\nPages requested: {}\nRapidAPI requests remaining: {}",
                outcome.newly_suppressed,
                outcome.already_decided,
                outcome.pages_requested,
                rate_limit,
            )
        }
    };

    ctx.say(reply).await?;

    Ok(())
}

#[poise::command(slash_command, rename = "opencritic_suppressed")]
pub async fn opencritic_suppressed(ctx: Context<'_>) -> Result<()> {
    let data = ctx.data();
    let rows = list_baseline_suppressed_opencritic(&data.db).await?;

    if rows.is_empty() {
        ctx.say("No baseline-suppressed OpenCritic games in the database.")
            .await?;
        return Ok(());
    }

    let mut body = format!(
        "Baseline-suppressed OpenCritic games ({} total, sorted by score)\n\n",
        rows.len()
    );
    for row in &rows {
        let url = row.url.as_deref().unwrap_or("");
        body.push_str(&format!(
            "- {} (score {:.1}, {} reviews) {}\n",
            row.name, row.score, row.review_count, url
        ));
    }

    let attachment =
        serenity::CreateAttachment::bytes(body.into_bytes(), "opencritic_suppressed.txt");
    let reply = CreateReply::default()
        .content(format!(
            "Baseline-suppressed OpenCritic games: {} total. See attachment.",
            rows.len()
        ))
        .attachment(attachment);
    ctx.send(reply).await?;

    Ok(())
}

#[poise::command(slash_command, rename = "opencritic_unsubscribe")]
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

#[poise::command(slash_command, rename = "opencritic_check")]
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

#[poise::command(slash_command, rename = "opencritic_status")]
pub async fn opencritic_status(ctx: Context<'_>) -> Result<()> {
    let data = ctx.data();
    let (baseline_suppressed, handled_games, subscriptions, announcement_deliveries) =
        count_opencritic_decisions(&data.db).await?;
    let rate_limit = data
        .opencritic_client
        .last_rate_limit_remaining()
        .unwrap_or_else(|| "unknown (no API call yet)".to_string());

    ctx.say(format!(
        "OpenCritic status:\n\
        Threshold: {:.1}\n\
        Minimum reviews: {}\n\
        Scan previous year: {}\n\
        Poll interval: {}\n\
        Last seen RapidAPI requests remaining: {}\n\
        Active subscriptions: {}\n\
        Baseline suppressed decisions: {}\n\
        Handled qualifying games: {}\n\
        Announcement deliveries: {}",
        data.config.opencritic_threshold,
        data.config.opencritic_min_reviews,
        data.config.opencritic_scan_previous_year,
        humanize_duration(data.config.opencritic_poll_interval),
        rate_limit,
        subscriptions,
        baseline_suppressed,
        handled_games,
        announcement_deliveries,
    ))
    .await?;

    Ok(())
}
