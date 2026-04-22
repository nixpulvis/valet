//! In-process backend: the shim is the whole server.
//!
//! Holds a [`valetd::DaemonHandler`] directly, skipping the Unix socket
//! entirely. The handler manages its own idle reaper, so cached keys are
//! dropped on inactivity even without a separate daemon process.

use std::io;
use std::sync::Arc;
use valetd::{DaemonHandler, Handler, Request, Response, request::Frame};

use super::Backend;

pub(crate) struct EmbeddedBackend {
    handler: Arc<DaemonHandler>,
}

impl EmbeddedBackend {
    pub(crate) async fn build() -> Result<Self, String> {
        let handler = DaemonHandler::from_env().await?;
        Ok(Self { handler })
    }
}

impl Backend for EmbeddedBackend {
    async fn round_trip(&self, request_bytes: &[u8]) -> io::Result<Vec<u8>> {
        let req = Request::decode(request_bytes)?;
        let resp: Response = self.handler.handle(req).await?;
        Ok(resp.encode())
    }
}
