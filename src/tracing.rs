use std::panic;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use tracing_subscriber::EnvFilter;

use crate::config::{Config, LogFormat};

pub use tracing::{error, info};

const DEFAULT_FILTER: &str = "info,jeff=debug,serenity=warn,sqlx=warn";

pub fn initialize_tracing(config: &Config) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    let builder = tracing_subscriber::fmt()
        .with_target(true)
        .with_env_filter(filter);

    match config.log_format {
        LogFormat::Pretty => builder.pretty().init(),
        LogFormat::Json => builder
            .json()
            .with_current_span(true)
            .with_span_list(false)
            .flatten_event(true)
            .init(),
    }

    install_panic_hook();
}

fn install_panic_hook() {
    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        tracing::error!(
            panic.location = %location,
            panic.payload = %payload,
            "panic"
        );
        default_hook(info);
    }));
}

pub fn short_id() -> String {
    static COUNTER: OnceLock<AtomicU64> = OnceLock::new();
    let counter = COUNTER.get_or_init(|| {
        let seed = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0);
        AtomicU64::new(seed)
    });
    let n = counter.fetch_add(1, Ordering::Relaxed);
    format!("{:06x}", n & 0xFF_FFFF)
}
