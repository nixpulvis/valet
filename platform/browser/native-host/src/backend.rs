//! Transport backend for the native-host shim.
//!
//! The shim hands each inbound request's raw bitcode bytes to a `Backend`
//! and expects raw bitcode response bytes in return. Two implementations,
//! selected at compile time:
//!
//! * [`SocketBackend`] (default) — connects to a sibling `valetd` over
//!   its Unix socket, auto-spawning the daemon if the socket is missing.
//!   Forwards bytes without decoding them.
//! * [`EmbeddedBackend`] (`--features embedded`) — owns a
//!   [`valetd::DaemonHandler`] in-process, so the shim is the whole
//!   server. Decodes the bitcode, dispatches, and re-encodes.
//!
//! [`Active`] aliases whichever variant the active feature selects.

use std::future::Future;
use std::io;

pub(crate) trait Backend: Send + Sync {
    /// Send one request payload and return the reply payload. Both are
    /// raw bitcode bytes (no framing, no base64).
    fn round_trip(&self, request_bytes: &[u8])
    -> impl Future<Output = io::Result<Vec<u8>>> + Send;
}

#[cfg(not(feature = "embedded"))]
mod socket;
#[cfg(not(feature = "embedded"))]
pub(crate) use socket::SocketBackend as Active;

#[cfg(feature = "embedded")]
mod embedded;
#[cfg(feature = "embedded")]
pub(crate) use embedded::EmbeddedBackend as Active;

/// Build the backend selected by the compile-time feature set.
pub(crate) async fn build() -> Result<Active, String> {
    Active::build().await
}
