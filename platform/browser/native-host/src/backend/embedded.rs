//! In-process backend: the shim is the whole server.
//!
//! Holds a [`valetd::DaemonHandler`] directly, skipping the Unix socket
//! entirely. Also spawns the same idle reaper the `valetd` binary runs,
//! so cached keys are dropped on inactivity even without a separate
//! daemon process.

use std::io;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;
use valetd::{DaemonHandler, Handler, Request, Response, request::Frame};

use super::Backend;

const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);

pub(crate) struct EmbeddedBackend {
    handler: Arc<DaemonHandler>,
}

impl EmbeddedBackend {
    pub(crate) async fn build() -> Result<Self, String> {
        let handler = Arc::new(DaemonHandler::from_env().await?);
        spawn_reaper(handler.clone());
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

fn spawn_reaper(handler: Arc<DaemonHandler>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(IDLE_CHECK_INTERVAL).await;
            if handler.reap_if_idle(IDLE_TIMEOUT).await {
                info!("idle timeout, locked all users");
            }
        }
    });
}
