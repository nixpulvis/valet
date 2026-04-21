//! Remote [`Handler`] impl that forwards requests to a running `valetd`
//! over its Unix socket.
//!
//! The client is stateless: every record-touching request names its target
//! `username` (and, where relevant, `lot`) explicitly. Session state lives
//! in `valetd` — which users are unlocked and which lot keys are cached,
//! scoped by `(username, lot)` and cleared by the idle reaper or explicit
//! [`Request::Lock`] / [`Request::LockAll`] calls.
//!
//! The stream is guarded by a mutex so concurrent [`Handler::handle`]
//! callers serialize at the socket; without it, two send+recv pairs could
//! interleave and the length-prefixed frame parser would trip
//! `MAX_FRAME_LEN` on the misaligned read.

use crate::request::{Frame, Request, Response};
use crate::server::Handler;
use std::{io, path::Path};
use tokio::net::UnixStream;
use tokio::sync::Mutex;

pub struct Client {
    stream: Mutex<UnixStream>,
}

impl Client {
    pub async fn connect(path: &Path) -> io::Result<Self> {
        Ok(Client {
            stream: Mutex::new(UnixStream::connect(path).await?),
        })
    }
}

impl Handler for Client {
    async fn handle(&self, req: Request) -> io::Result<Response> {
        let mut stream = self.stream.lock().await;
        req.send_async(&mut *stream).await?;
        Response::recv_async(&mut *stream).await
    }
}
