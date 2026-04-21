//! Transport backends for the native-host shim.
//!
//! The shim hands each inbound request's raw bitcode bytes to a [`Backend`]
//! and expects raw bitcode response bytes in return. Both implementations
//! are always compiled in:
//!
//! * [`SocketBackend`] — forwards bytes to a running `valetd` over its
//!   Unix socket, without decoding them.
//! * [`EmbeddedBackend`] — owns a [`valetd::DaemonHandler`] directly, so
//!   the shim is the whole server. Pulls in SQLite and crypto.
//!
//! At startup, [`build`] picks one based on `VALET_BACKEND`:
//!
//! * unset / `auto` — probe the socket; on success use it, on miss fall
//!   back to embedded.
//! * `socket` — use the socket, fail if nothing is listening.
//! * `embedded` — skip the probe, always embed.
//!
//! Adding a new RPC variant touches neither backend.
//!
//! TODO: if the user started the shim in embedded mode and later runs
//! `valetd`, both will hold the same SQLite database. Writes serialise,
//! but unlocked-key caches are independent, so changes made through one
//! side are invisible to the other until its cache is rebuilt. Options:
//! a background re-probe that switches backends on the fly; a DB-level
//! lock so the later opener fails fast; or documenting that users pick
//! one mode per session. Revisit before shipping embedded mode widely.

use std::future::Future;
use std::io;
use valetd::socket;

mod embedded;
mod socket_relay;

pub(crate) use embedded::EmbeddedBackend;
pub(crate) use socket_relay::SocketBackend;

pub(crate) trait Backend: Send + Sync {
    /// Send one request payload and return the reply payload. Both are
    /// raw bitcode bytes (no framing, no base64).
    fn round_trip(&self, request_bytes: &[u8])
    -> impl Future<Output = io::Result<Vec<u8>>> + Send;
}

/// Both backends live behind a single enum so the main loop can hold one
/// concrete type and stay free of the `async fn in dyn trait` ceremony.
pub(crate) enum Active {
    Socket(SocketBackend),
    Embedded(EmbeddedBackend),
}

impl Backend for Active {
    async fn round_trip(&self, request_bytes: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::Socket(b) => b.round_trip(request_bytes).await,
            Self::Embedded(b) => b.round_trip(request_bytes).await,
        }
    }
}

enum Mode {
    Auto,
    Socket,
    Embedded,
}

fn mode_from_env() -> Result<Mode, String> {
    match std::env::var("VALET_BACKEND").as_deref() {
        Ok("") | Ok("auto") | Err(_) => Ok(Mode::Auto),
        Ok("socket") => Ok(Mode::Socket),
        Ok("embedded") => Ok(Mode::Embedded),
        Ok(other) => Err(format!(
            "VALET_BACKEND={other}: expected 'auto', 'socket', or 'embedded'"
        )),
    }
}

/// Resolve the runtime backend. See the module docs for the selection rule.
pub(crate) async fn build() -> Result<Active, String> {
    let mode = mode_from_env()?;
    if matches!(mode, Mode::Embedded) {
        return EmbeddedBackend::build().await.map(Active::Embedded);
    }

    let socket_path = socket::path();
    let probe = SocketBackend::try_connect(&socket_path)
        .await
        .map_err(|e| format!("socket error at {}: {e}", socket_path.display()))?;

    match (mode, probe) {
        (_, Some(b)) => Ok(Active::Socket(b)),
        (Mode::Socket, None) => Err(format!(
            "VALET_BACKEND=socket but no daemon is listening at {}",
            socket_path.display()
        )),
        (Mode::Auto, None) => EmbeddedBackend::build().await.map(Active::Embedded),
        (Mode::Embedded, None) => unreachable!("handled above"),
    }
}
