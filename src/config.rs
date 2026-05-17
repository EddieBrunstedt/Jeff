use std::time::Duration;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

#[derive(Debug, Clone, Copy)]
pub enum LogFormat {
    Pretty,
    Json,
}

impl LogFormat {
    fn from_env() -> Result<Self> {
        let value = std::env::var("LOG_FORMAT").unwrap_or_else(|_| "json".to_string());
        match value.to_ascii_lowercase().as_str() {
            "json" => Ok(LogFormat::Json),
            "pretty" => Ok(LogFormat::Pretty),
            other => bail!("LOG_FORMAT must be 'json' or 'pretty', got '{}'", other),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub discord_token: String,
    pub guild_id: serenity::GuildId,
    pub log_format: LogFormat,
    pub rapidapi_key: String,
    pub database_url: String,
    pub opencritic_poll_interval: Duration,
    pub enable_debug_post: bool,
    pub disable_polling: bool,
    pub refresh_cooldown: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let discord_token = get_required_env_var("DISCORD_TOKEN")?;

        let guild_id = get_required_env_var("GUILD_ID")?
            .parse::<u64>()
            .map(serenity::GuildId::new)
            .context("GUILD_ID must be a valid Discord guild ID")?;

        let log_format = LogFormat::from_env()?;

        let rapidapi_key = get_required_env_var("RAPIDAPI_KEY")?;

        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://jeff.sqlite?mode=rwc".to_string());

        let opencritic_poll_interval_seconds: u64 =
            get_required_env_var("OPENCRITIC_POLL_INTERVAL_SECONDS")?
                .parse()
                .context("OPENCRITIC_POLL_INTERVAL_SECONDS must be a valid integer")?;

        let enable_debug_post = std::env::var("JEFF_ENABLE_DEBUG_POST")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .context("JEFF_ENABLE_DEBUG_POST must be a valid boolean")?;

        let disable_polling = std::env::var("JEFF_DISABLE_POLLING")
            .unwrap_or_else(|_| "false".to_string())
            .parse()
            .context("JEFF_DISABLE_POLLING must be a valid boolean")?;

        let refresh_cooldown_seconds: u64 = std::env::var("JEFF_REFRESH_COOLDOWN_SECONDS")
            .unwrap_or_else(|_| "60".to_string())
            .parse()
            .context("JEFF_REFRESH_COOLDOWN_SECONDS must be a non-negative integer")?;

        Ok(Config {
            discord_token,
            guild_id,
            log_format,
            rapidapi_key,
            database_url,
            opencritic_poll_interval: Duration::from_secs(opencritic_poll_interval_seconds),
            enable_debug_post,
            disable_polling,
            refresh_cooldown: Duration::from_secs(refresh_cooldown_seconds),
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
