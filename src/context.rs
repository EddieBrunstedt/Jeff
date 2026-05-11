use crate::{config::Config, opencritic::OpenCriticClient, poller::LastPollSnapshot};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};

pub struct Data {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub opencritic_client: OpenCriticClient,
    pub started_at: DateTime<Utc>,
    pub last_poll: Arc<Mutex<Option<LastPollSnapshot>>>,
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;
