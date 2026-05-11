use std::time::Duration;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

#[derive(Debug, Clone)]
pub struct Config {
    pub _discord_character_limit: usize,
    pub discord_token: String,
    pub guild_id: Option<serenity::GuildId>,
    pub pretty_log: bool,
    pub rapidapi_key: String,
    pub database_url: String,
    pub opencritic_threshold: f64,
    pub opencritic_min_reviews: i64,
    pub opencritic_scan_previous_year: bool,
    pub opencritic_poll_interval: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let _discord_character_limit: usize = std::env::var("DISCORD_CHARACTER_LIMIT")
            .ok()
            .as_deref()
            .unwrap_or("2000")
            .parse()
            .context("DISCORD_CHARACTER_LIMIT must be a valid positive integer")?;

        let discord_token = get_required_env_var("DISCORD_TOKEN")?;

        let guild_id = std::env::var("GUILD_ID")
            .ok()
            .map(|s| {
                s.parse::<u64>()
                    .map(serenity::GuildId::new)
                    .context("GUILD_ID must be a valid Discord guild ID")
            })
            .transpose()?;

        let pretty_log = std::env::var("PRETTY_LOG")
            .unwrap_or("false".to_string())
            .parse()?;

        let rapidapi_key = get_required_env_var("RAPIDAPI_KEY")?;

        let database_url = std::env::var("DATABASE_URL")
            .unwrap_or_else(|_| "sqlite://jeff.sqlite?mode=rwc".to_string());

        let opencritic_threshold = std::env::var("OPENCRITIC_THRESHOLD")
            .ok()
            .as_deref()
            .unwrap_or("85")
            .parse()
            .context("OPENCRITIC_THRESHOLD must be a valid number")?;

        let opencritic_min_reviews = std::env::var("OPENCRITIC_MIN_REVIEWS")
            .ok()
            .as_deref()
            .unwrap_or("10")
            .parse()
            .context("OPENCRITIC_MIN_REVIEWS must be a valid integer")?;

        let opencritic_scan_previous_year = std::env::var("OPENCRITIC_SCAN_PREVIOUS_YEAR")
            .unwrap_or("true".to_string())
            .parse()?;

        let opencritic_poll_interval_seconds: u64 =
            std::env::var("OPENCRITIC_POLL_INTERVAL_SECONDS")
                .ok()
                .as_deref()
                .unwrap_or("21600")
                .parse()
                .context("OPENCRITIC_POLL_INTERVAL_SECONDS must be a valid integer")?;

        Ok(Config {
            _discord_character_limit,
            discord_token,
            guild_id,
            pretty_log,
            rapidapi_key,
            database_url,
            opencritic_threshold,
            opencritic_min_reviews,
            opencritic_scan_previous_year,
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
