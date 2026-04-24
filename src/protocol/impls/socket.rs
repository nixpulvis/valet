//! Unix-socket protocol: [`Client<Socket>`] forwards frames over a
//! connected [`UnixStream`]; [`Server<Socket>`] listens on a
//! [`UnixListener`] and hands each frame to whatever [`Handler`] the
//! daemon gave it. Plus the socket-path resolution helpers
//! ([`path`] / [`default_path`]) that the daemon and every socket
//! client share.
//!
//! [`Client<Socket>`]: crate::protocol::Client
//! [`Server<Socket>`]: crate::protocol::Server
//! [`Handler`]: crate::protocol::Handler
//! [`UnixStream`]: tokio::net::UnixStream
//! [`UnixListener`]: tokio::net::UnixListener

use std::path::PathBuf;

/// The socket path the daemon binds to and clients connect to. Honors
/// `$VALET_SOCKET` if set; otherwise falls back to [`default_path`].
/// Use this in preference to `default_path` so the env override is
/// applied uniformly across the daemon, shim, and FFI clients.
pub fn path() -> PathBuf {
    std::env::var_os("VALET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(default_path)
}

/// Default socket path: `$XDG_RUNTIME_DIR/valet/valet.sock`, falling
/// back to `$TMPDIR/valet/valet.sock` (and `/tmp/valet/valet.sock` if
/// `TMPDIR` is unset). Returns an absolute path; the parent directory
/// is not created here.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("valet").join("valet.sock")
}

/// Wire-protocol marker for the length-prefixed bitcode framing carried
/// over a Unix domain socket.
pub struct Socket;

#[cfg(feature = "protocol-socket")]
pub use impl_protocol::{SocketClient, SocketServer};

#[cfg(feature = "protocol-socket")]
impl crate::protocol::Protocol for Socket {
    type Client = impl_protocol::SocketClient;
    type Server = impl_protocol::SocketServer;
}

#[cfg(feature = "protocol-socket")]
mod impl_protocol {
    use super::Socket;
    use crate::protocol::frame::Frame;
    use crate::protocol::message::{Request, Response};
    use crate::protocol::{Client, Handler, Server};
    use std::io;
    use std::path::Path;
    use std::sync::Arc;
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::Mutex;

    /// State behind [`Client<Socket>`]. A connected stream guarded by
    /// a mutex so concurrent [`Handler::handle`] callers serialize at
    /// the socket; without it, two send+recv pairs could interleave
    /// and the length-prefixed frame parser would trip `MAX_FRAME_LEN`
    /// on the misaligned read.
    pub struct SocketClient {
        stream: Mutex<UnixStream>,
    }

    /// State behind [`Server<Socket>`]. Holds the bound listener.
    pub struct SocketServer {
        listener: UnixListener,
    }

    impl Client<Socket> {
        /// Connect to a daemon listening at `path`.
        pub async fn connect(path: &Path) -> io::Result<Self> {
            let stream = UnixStream::connect(path).await?;
            Ok(Client {
                inner: SocketClient {
                    stream: Mutex::new(stream),
                },
            })
        }
    }

    impl Handler for Client<Socket> {
        async fn handle(&self, req: Request) -> io::Result<Response> {
            let mut stream = self.inner.stream.lock().await;
            req.send_async(&mut *stream).await?;
            Response::recv_async(&mut *stream).await
        }
    }

    impl Server<Socket> {
        /// Bind a new Unix-socket listener at `path`. Creates the
        /// parent directory if needed and removes any stale socket
        /// file left over from a crashed prior run. If a live daemon
        /// is already listening, returns [`io::ErrorKind::AddrInUse`]
        /// rather than stealing the path.
        pub async fn bind(path: &Path) -> io::Result<Self> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            // Probe before unlink: if a live daemon answers a connect,
            // another instance owns this path; don't steal its accepts.
            // Otherwise the file is either absent or a stale socket
            // from a crashed run, so clear it and bind fresh.
            if UnixStream::connect(path).await.is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("another daemon is listening at {}", path.display()),
                ));
            }
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            let listener = UnixListener::bind(path)?;
            Ok(Server {
                inner: SocketServer { listener },
            })
        }

        /// Accept connections and dispatch every incoming request
        /// through `handler`. Never returns under normal operation.
        pub async fn serve<H: Handler + 'static>(self, handler: Arc<H>) -> io::Result<()> {
            loop {
                let (conn, _) = match self.inner.listener.accept().await {
                    Ok(x) => x,
                    Err(err) => {
                        tracing::warn!("accept failed: {err}");
                        continue;
                    }
                };
                let handler = handler.clone();
                tokio::spawn(async move {
                    if let Err(err) = serve_conn(conn, handler).await {
                        tracing::warn!("connection ended: {err}");
                    }
                });
            }
        }
    }

    async fn serve_conn<H: Handler>(mut conn: UnixStream, handler: Arc<H>) -> io::Result<()> {
        loop {
            let req = match Request::recv_async(&mut conn).await {
                Ok(r) => r,
                // Clean EOF when the client closes the socket.
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(e) => return Err(e),
            };
            let response: Response = handler.handle(req).await?;
            response.send_async(&mut conn).await?;
        }
    }
}
