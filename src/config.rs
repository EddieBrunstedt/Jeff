use std::time::Duration;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

#[derive(Debug, Clone)]
pub struct Config {
    pub discord_token: String,
    pub guild_id: serenity::GuildId,
    pub pretty_log: bool,
    pub rapidapi_key: String,
    pub database_url: String,
    pub opencritic_poll_interval: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let discord_token = get_required_env_var("DISCORD_TOKEN")?;

        let guild_id = get_required_env_var("GUILD_ID")?
            .parse::<u64>()
            .map(serenity::GuildId::new)
            .context("GUILD_ID must be a valid Discord guild ID")?;

        let pretty_log = std::env::var("PRETTY_LOG")
            .unwrap_or("false".to_string())
            .parse()?;

        let rapidapi_key = get_required_env_var("RAPIDAPI_KEY")?;

        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://jeff.sqlite?mode=rwc".to_string());

        let opencritic_poll_interval_seconds: u64 =
            get_required_env_var("OPENCRITIC_POLL_INTERVAL_SECONDS")?
                .parse()
                .context("OPENCRITIC_POLL_INTERVAL_SECONDS must be a valid integer")?;

        Ok(Config {
            discord_token,
            guild_id,
            pretty_log,
            rapidapi_key,
            database_url,
            opencritic_poll_interval: Duration::from_secs(opencritic_poll_interval_seconds),
        })
    }
}

fn get_required_env_var(name: &str) -> Result<String> {
    let value = std::env::var(name)
        .with_context(|| format!("Missing required {name} environment variable"))?;

    if value.is_empty() {
        bail!("{} environment variable is set but empty", name);
    }

    Ok(value)
}
