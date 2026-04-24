//! Native-messaging transport entry point: read the browser's stdio
//! envelope and serve it from either a local [`EmbeddedHandler`] or
//! a [`SocketClient`] relay, selected by `VALET_BACKEND`.
//!
//! Each backend is gated on its protocol feature. Requesting a
//! backend that wasn't compiled in fails at runtime with a clear
//! message; `Backend::Auto` tries the socket first (when available)
//! and falls back to embedded (when available).
//!
//! [`EmbeddedHandler`]: valet::protocol::EmbeddedHandler
//! [`SocketClient`]: valet::protocol::SocketClient

use std::sync::Arc;
use valet::protocol::{NativeMessageServer, Serve};

#[cfg(any(feature = "protocol-socket", feature = "protocol-embedded"))]
use tracing::info;
#[cfg(feature = "protocol-socket")]
use valet::protocol::{SocketClient, socket};

use super::Backend;

pub(crate) async fn run(backend: Backend) -> Result<(), String> {
    match backend {
        Backend::Embedded => run_embedded().await,
        Backend::Socket => run_socket_relay().await,
        Backend::Auto => run_auto().await,
    }
}

#[cfg(feature = "protocol-embedded")]
async fn run_embedded() -> Result<(), String> {
    let handler = super::build_embedded_handler().await?;
    info!(backend = "embedded", "selected");
    serve("embedded", handler).await
}

#[cfg(not(feature = "protocol-embedded"))]
async fn run_embedded() -> Result<(), String> {
    Err("embedded backend is disabled in this build \
         (requires feature `protocol-embedded`)"
        .to_string())
}

#[cfg(feature = "protocol-socket")]
async fn run_socket_relay() -> Result<(), String> {
    let path = socket::path();
    let client = SocketClient::connect(&path)
        .await
        .map_err(|e| format!("connect {}: {e}", path.display()))?;
    info!(backend = "socket", "selected");
    serve("socket", Arc::new(client)).await
}

#[cfg(not(feature = "protocol-socket"))]
async fn run_socket_relay() -> Result<(), String> {
    Err("socket backend is disabled in this build \
         (requires feature `protocol-socket`)"
        .to_string())
}

#[cfg(all(feature = "protocol-socket", feature = "protocol-embedded"))]
async fn run_auto() -> Result<(), String> {
    let path = socket::path();
    match SocketClient::connect(&path).await {
        Ok(client) => {
            info!(backend = "socket", path = %path.display(), "selected");
            serve("socket", Arc::new(client)).await
        }
        Err(e) => {
            info!(
                backend = "embedded",
                path = %path.display(),
                reason = %e,
                "selected (no daemon at socket)",
            );
            let handler = super::build_embedded_handler().await?;
            serve("embedded", handler).await
        }
    }
}

#[cfg(all(feature = "protocol-socket", not(feature = "protocol-embedded")))]
async fn run_auto() -> Result<(), String> {
    run_socket_relay().await
}

#[cfg(all(not(feature = "protocol-socket"), feature = "protocol-embedded"))]
async fn run_auto() -> Result<(), String> {
    run_embedded().await
}

#[cfg(not(any(feature = "protocol-socket", feature = "protocol-embedded")))]
async fn run_auto() -> Result<(), String> {
    Err("no native-messaging backend is available in this build \
         (enable `protocol-embedded` and/or `protocol-socket`)"
        .to_string())
}

#[cfg_attr(
    not(any(feature = "protocol-embedded", feature = "protocol-socket")),
    allow(dead_code)
)]
async fn serve<H: valet::SendHandler + 'static>(
    tag: &'static str,
    handler: Arc<H>,
) -> Result<(), String> {
    NativeMessageServer::from_stdio(tag)
        .serve(handler)
        .await
        .map_err(|e| format!("serve: {e}"))
}
