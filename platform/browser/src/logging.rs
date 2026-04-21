//! Shared tracing setup for the popup and background entry points.
//!
//! Debug builds capture `TRACE` and above; release builds capture `INFO`
//! and above. Output goes to the browser console via [`tracing_web`].

use std::sync::OnceLock;

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::prelude::*;
use tracing_web::MakeWebConsoleWriter;

static INIT: OnceLock<()> = OnceLock::new();

/// Initialize tracing for the given subsystem (e.g. `"background"` or `"popup"`).
///
/// Safe to call multiple times — only the first call takes effect.
pub fn init(subsystem: &str) {
    INIT.get_or_init(|| {
        let level = if cfg!(debug_assertions) {
            LevelFilter::TRACE
        } else {
            LevelFilter::INFO
        };
        let fmt_layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(MakeWebConsoleWriter::new());
        tracing_subscriber::registry()
            .with(level)
            .with(fmt_layer)
            .init();
        tracing::info!(subsystem, "valet wasm initialized");
    });
}
