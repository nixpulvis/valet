//! Socket-transport entry point: bind `$VALET_SOCKET` (or the default
//! path) and serve a local [`EmbeddedHandler`] on it.
//!
//! [`EmbeddedHandler`]: valet::protocol::EmbeddedHandler

use tracing::info;
use valet::protocol::{Serve, SocketServer, socket};

use super::build_embedded_handler;

pub(crate) async fn run() -> Result<(), String> {
    let path = socket::path();
    let handler = build_embedded_handler().await?;
    let server = SocketServer::bind(&path)
        .await
        .map_err(|e| format!("bind {}: {e}", path.display()))?;
    info!(path = %path.display(), "listening");
    server
        .serve(handler)
        .await
        .map_err(|e| format!("serve: {e}"))
}
