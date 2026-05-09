use crate::config::Config;
use std::sync::Arc;

pub struct Data {
    pub _config: Arc<Config>,
}

pub type Error = anyhow::Error;
pub type Context<'a> = poise::Context<'a, Data, Error>;
