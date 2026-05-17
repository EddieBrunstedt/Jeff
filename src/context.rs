use crate::{
    opencritic::Client,
    poller::{LastPollSnapshot, RefreshThrottle},
};
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};

pub struct Data {
    pub db: SqlitePool,
    pub opencritic_client: Client,
    pub started_at: DateTime<Utc>,
    pub last_poll: Arc<Mutex<Option<LastPollSnapshot>>>,
    pub refresh_throttle: Arc<RefreshThrottle>,
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;
