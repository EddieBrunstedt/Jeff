use crate::{config::Config, opencritic::OpenCriticClient};
use sqlx::SqlitePool;
use std::sync::Arc;

pub struct Data {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub opencritic_client: OpenCriticClient,
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;
