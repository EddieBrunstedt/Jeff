use std::time::Instant;

use tracing::info;

pub struct CommandTimer {
    started: Instant,
    completed: bool,
}

impl CommandTimer {
    pub fn start() -> Self {
        info!("command received");
        Self {
            started: Instant::now(),
            completed: false,
        }
    }

    pub fn complete(mut self, outcome: &'static str) {
        info!(
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            outcome,
            "command complete",
        );
        self.completed = true;
    }
}

impl Drop for CommandTimer {
    fn drop(&mut self) {
        if !self.completed {
            info!(
                elapsed_ms = self.started.elapsed().as_millis() as u64,
                outcome = "error",
                "command complete",
            );
        }
    }
}
