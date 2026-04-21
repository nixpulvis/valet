//! Shared `tracing-subscriber` installer.
//!
//! [`init`] routes `tracing` events to stderr using the default `fmt`
//! layer; each line includes the event target, so messages from
//! different modules are naturally distinguished.
//!
//! Filter level comes from `$VALET_LOG`, then `$RUST_LOG`, then `info`.
//! Calls after the first are no-ops.

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Install a stderr subscriber on the current process. Idempotent.
pub fn init() {
    let filter = EnvFilter::try_from_env("VALET_LOG")
        .or_else(|_| EnvFilter::try_from_default_env())
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let layer = fmt::layer().with_writer(std::io::stderr);
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(layer)
        .try_init();
}
