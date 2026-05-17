use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;
use tracing::{error, field, info, warn};

use crate::opencritic::Client;

use super::embed::build_announcement_embed;

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

    pub fn cooldown_remaining(&self, game_id: i64) -> Option<Duration> {
        if self.cooldown.is_zero() {
            return None;
        }

        let map = self.last_press.lock().unwrap();
        let elapsed = map.get(&game_id)?.elapsed();
        self.cooldown.checked_sub(elapsed)
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

pub fn build_refresh_action_row(game_id: i64) -> serenity::CreateActionRow {
    serenity::CreateActionRow::Buttons(vec![
        serenity::CreateButton::new(format!("{REFRESH_BUTTON_PREFIX}{game_id}"))
            .label("Refresh")
            .style(serenity::ButtonStyle::Secondary),
    ])
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
    client: &Client,
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

    if let Some(remaining) = throttle.cooldown_remaining(game_id) {
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
                .embed(build_announcement_embed(&game))
                .components(vec![build_refresh_action_row(game.id)]),
        )
        .await
        .context("Failed to edit refreshed announcement")?;

    throttle.record(game_id);
    info!("Refresh completed");

    Ok(())
}
