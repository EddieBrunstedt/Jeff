use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use chrono::Utc;
use poise::serenity_prelude as serenity;
use tracing::{error, info};

mod commands;
mod config;
mod context;
mod db;
mod discord;
mod opencritic;
mod poller;
mod tracing;
mod util;

use config::Config;
use context::Data;
use opencritic::Client;

#[tokio::main]
async fn main() {
    let config = Config::from_env().expect("Failed to parse configuration");

    tracing::initialize_tracing(&config);

    if let Err(e) = run_bot(config).await {
        error!(error = ?e, "Bot exited with error");
        std::process::exit(1);
    }
}

async fn run_bot(config: Config) -> Result<()> {
    info!("Starting Discord bot");

    let config = Arc::new(config);

    let framework = {
        let config = Arc::clone(&config);

        let mut jeff_command = commands::jeff::jeff();
        if config.enable_debug_post {
            jeff_command.subcommands.push(commands::jeff::debug_post());
            info!("JEFF_ENABLE_DEBUG_POST is true; registering /jeff debug_post subcommand");
        }

        poise::Framework::builder()
            .options(poise::FrameworkOptions {
                commands: vec![jeff_command],
                event_handler: |ctx, event, _framework, data| {
                    Box::pin(handle_event(ctx, event, data))
                },
                ..Default::default()
            })
            .setup(move |ctx, _ready, framework| {
                Box::pin(build_data(ctx, framework, Arc::clone(&config)))
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

async fn handle_event(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    data: &Data,
) -> Result<()> {
    if let serenity::FullEvent::InteractionCreate { interaction } = event
        && let serenity::Interaction::Component(component) = interaction
        && component
            .data
            .custom_id
            .starts_with(discord::refresh::REFRESH_BUTTON_PREFIX)
    {
        discord::refresh::handle_refresh_button(
            ctx,
            component,
            &data.opencritic_client,
            &data.refresh_throttle,
        )
        .await?;
    }
    Ok(())
}

async fn build_data(
    ctx: &serenity::Context,
    framework: &poise::Framework<Data, context::Error>,
    config: Arc<Config>,
) -> Result<Data> {
    poise::builtins::register_in_guild(ctx, &framework.options().commands, config.guild_id)
        .await
        .context("Failed to register commands in guild")?;
    info!("Registered commands in guild {}", config.guild_id);

    let db = db::connect_and_migrate(&config.database_url).await?;
    let opencritic_client = Client::new(config.rapidapi_key.clone());
    let last_poll = Arc::new(Mutex::new(None));
    let refresh_throttle = Arc::new(discord::refresh::RefreshThrottle::new(
        config.refresh_cooldown,
    ));

    if config.disable_polling {
        info!("JEFF_DISABLE_POLLING is true; background poller will not be started");
    } else {
        poller::spawn_background_poller(
            Arc::clone(&ctx.http),
            db.clone(),
            opencritic_client.clone(),
            Arc::clone(&config),
            Arc::clone(&last_poll),
        );
        info!("Started OpenCritic background poller");
    }

    info!("Bot initialization complete");

    Ok(Data {
        db,
        opencritic_client,
        started_at: Utc::now(),
        last_poll,
        refresh_throttle,
    })
}
