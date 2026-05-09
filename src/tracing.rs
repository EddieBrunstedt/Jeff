use crate::config::Config;
use tracing_subscriber::EnvFilter;

pub use tracing::{error, info};

pub fn initialize_tracing(config: &Config) {
    let common_subscriber = tracing_subscriber::fmt()
        .with_target(true)
        .with_env_filter(EnvFilter::from_default_env());

    if config.pretty_log {
        common_subscriber.pretty().init();
    } else {
        common_subscriber.init();
    }
}
