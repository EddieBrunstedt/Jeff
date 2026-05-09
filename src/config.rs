use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

#[derive(Debug, Clone)]
pub struct Config {
    pub _discord_character_limit: usize,
    pub discord_token: String,
    pub guild_id: Option<serenity::GuildId>,
    pub pretty_log: bool,
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
            .parse()
            .unwrap_or(false);

        Ok(Config {
            _discord_character_limit,
            discord_token,
            guild_id,
            pretty_log,
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
