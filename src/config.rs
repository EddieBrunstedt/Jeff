use std::{fmt, str::FromStr, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use poise::serenity_prelude as serenity;

#[derive(Debug, Clone, Copy)]
pub enum LogFormat {
    Pretty,
    Json,
}

impl FromStr for LogFormat {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "json" => Ok(LogFormat::Json),
            "pretty" => Ok(LogFormat::Pretty),
            other => bail!("expected 'json' or 'pretty', got {other:?}"),
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
        let discord_token = env_required::<String>("DISCORD_TOKEN")?;
        let guild_id = serenity::GuildId::new(env_required::<u64>("GUILD_ID")?);
        let log_format = env_or("LOG_FORMAT", LogFormat::Json)?;
        let rapidapi_key = env_required::<String>("RAPIDAPI_KEY")?;
        let database_url = env_or("DATABASE_URL", "sqlite://jeff.sqlite?mode=rwc".to_string())?;
        let opencritic_poll_interval_seconds =
            env_required::<u64>("OPENCRITIC_POLL_INTERVAL_SECONDS")?;
        let enable_debug_post = env_or("JEFF_ENABLE_DEBUG_POST", false)?;
        let disable_polling = env_or("JEFF_DISABLE_POLLING", false)?;
        let refresh_cooldown_seconds = env_or("JEFF_REFRESH_COOLDOWN_SECONDS", 60_u64)?;

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

fn env_required<T>(name: &str) -> Result<T>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    let value = std::env::var(name)
        .with_context(|| format!("Missing required {name} environment variable"))?;

    if value.is_empty() {
        bail!("{name} environment variable is set but empty");
    }

    parse_env_value(name, &value)
}

fn env_or<T>(name: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(default),
        Err(err) => bail!("Failed to read {name} environment variable: {err}"),
    };

    parse_env_value(name, &value)
}

fn parse_env_value<T>(name: &str, value: &str) -> Result<T>
where
    T: FromStr,
    T::Err: fmt::Display,
{
    value
        .parse::<T>()
        .map_err(|err| anyhow!("{name} has invalid value {value:?}: {err}"))
}
