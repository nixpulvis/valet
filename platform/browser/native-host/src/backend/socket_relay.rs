//! Unix-socket backend: forwards request bytes to a running `valetd`.
//!
//! This backend does not start the daemon. If no socket is reachable, the
//! shim falls back to the in-process [`super::EmbeddedBackend`] instead.
//! See [`super`] for the runtime selection rule.

use std::io;
use std::path::Path;
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use valetd::request::{recv_frame_async, send_frame_async};

use super::Backend;

pub(crate) struct SocketBackend {
    stream: Mutex<UnixStream>,
}

impl SocketBackend {
    /// Try to connect to a daemon already listening on `path`. Returns
    /// `Ok(None)` if the socket is absent or refused — caller decides
    /// whether to fall back or surface the miss. `Err` is reserved for
    /// unexpected IO failures.
    pub(crate) async fn try_connect(path: &Path) -> io::Result<Option<Self>> {
        match UnixStream::connect(path).await {
            Ok(stream) => Ok(Some(Self {
                stream: Mutex::new(stream),
            })),
            Err(e) if is_missing(&e) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// True for the errno values `connect()` returns when nothing is listening
/// on the socket, as opposed to a real IO failure on an existing endpoint.
fn is_missing(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
    )
}

impl Backend for SocketBackend {
    async fn round_trip(&self, request_bytes: &[u8]) -> io::Result<Vec<u8>> {
        let mut stream = self.stream.lock().await;
        send_frame_async(&mut *stream, request_bytes).await?;
        recv_frame_async(&mut *stream).await
    }
}
