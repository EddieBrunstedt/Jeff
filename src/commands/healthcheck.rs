use std::time::Instant;

use anyhow::Result;
use chrono::Utc;

use crate::{
    context::Context,
    db::{count_opencritic_subscriptions, ping},
    util::humanize_duration,
};

#[poise::command(slash_command)]
pub async fn healthcheck(ctx: Context<'_>) -> Result<()> {
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

    let active_subscriptions = count_opencritic_subscriptions(&data.db).await?;

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
        Active subscriptions: {}\n\
        Last successful poll: {}",
        humanize_duration(uptime),
        db_status,
        latency_str,
        rate_limit,
        active_subscriptions,
        last_poll_str,
    ))
    .await?;

    Ok(())
}
