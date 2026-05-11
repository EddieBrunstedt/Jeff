use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use poise::serenity_prelude as serenity;
use tracing::{error, info};

mod commands;
mod config;
mod context;
mod db;
mod opencritic;
mod poller;
mod tracing;
mod util;

use config::Config;
use context::Data;
use opencritic::OpenCriticClient;

#[tokio::main]
async fn main() {
    let config = Config::from_env().expect("Failed to parse configuration");

    tracing::initialize_tracing(&config);

    if let Err(e) = run_bot(config).await {
        error!("{}", e.to_string());
        std::process::exit(1);
    }
}

async fn run_bot(config: Config) -> Result<()> {
    info!("Starting Discord bot");

    let config = Arc::new(config);

    let framework = {
        let config = Arc::clone(&config);

        poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: vec![commands::jeff::jeff()],
                ..Default::default()
            })
            .setup(move |ctx, _ready, framework| {
                Box::pin(initialize_bot_data(ctx, framework, Arc::clone(&config)))
            })
            .build()
    };

    let mut client = serenity::ClientBuilder::new(
        &config.discord_token,
        serenity::GatewayIntents::non_privileged(),
    )
    .framework(framework)
    .await
    .context("Failed to create Discord client")?;

    client
        .start()
        .await
        .context("Failed to start Discord bot")?;

    Ok(())
}

async fn initialize_bot_data(
    ctx: &serenity::Context,
    framework: &poise::Framework<Data, context::Error>,
    config: Arc<Config>,
) -> Result<Data> {
    poise::builtins::register_in_guild(ctx, &framework.options().commands, config.guild_id)
        .await
        .context("Failed to register commands in guild")?;
    info!("Registered commands in guild {}", config.guild_id);

    let db = db::connect_and_migrate(&config.database_url).await?;
    let opencritic_client = OpenCriticClient::new(config.rapidapi_key.clone());
    let last_poll = Arc::new(Mutex::new(None));

    poller::spawn_opencritic_background_poll(
        Arc::clone(&ctx.http),
        db.clone(),
        opencritic_client.clone(),
        Arc::clone(&config),
        Arc::clone(&last_poll),
    );
    info!("Started OpenCritic background poller");

    info!("Bot initialization complete");

    Ok(Data {
        db,
        opencritic_client,
        started_at: Utc::now(),
        last_poll,
    })
}
