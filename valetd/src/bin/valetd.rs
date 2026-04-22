//! `valetd` - the Valet daemon.
//!
//! Owns a single SQLite database, a cache of unlocked [`User`] / [`Lot`]
//! keys, and serves the [`valetd::Request`] / [`valetd::Response`] protocol
//! on a Unix socket. Cached keys are dropped after the idle window with no
//! activity (see [`valetd::server::IDLE_TIMEOUT`]); they are also dropped
//! when the process exits because [`valet::encrypt::Key`] is `ZeroizeOnDrop`.
//!
//! All request handling lives in [`valetd::server::DaemonHandler`]; this
//! binary is the Unix-socket transport around it.
//!
//! Socket path: `$VALET_SOCKET` if set, otherwise [`valetd::socket::default_path`].
//! Database path: `$VALET_DB` if set, otherwise [`valet::db::default_url`].
//!
//! [`User`]: valet::user::User
//! [`Lot`]: valet::Lot

use std::sync::Arc;
use tokio::net::{UnixListener, UnixStream};
use tracing::{error, info, warn};
use valetd::{
    DaemonHandler, Handler, Request, Response,
    request::Frame,
    socket,
};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    valet::logging::init();
    let socket_path = socket::path();

    let handler = match DaemonHandler::from_env().await {
        Ok(h) => h,
        Err(err) => {
            error!("{err}");
            std::process::exit(1);
        }
    };

    if let Some(parent) = socket_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            error!(
                path = %parent.display(),
                "failed to create socket directory: {err}"
            );
            std::process::exit(1);
        }
    }
    // A stale socket file from a crashed previous run would make bind() fail
    // with EADDRINUSE; remove it. If something is actually listening, the
    // bind below will still fail on the port-already-in-use race (not much
    // we can do from here).
    let _ = std::fs::remove_file(&socket_path);

    let listener = match UnixListener::bind(&socket_path) {
        Ok(l) => l,
        Err(err) => {
            error!(path = %socket_path.display(), "failed to bind: {err}");
            std::process::exit(1);
        }
    };
    info!(path = %socket_path.display(), "listening");

    loop {
        let (conn, _) = match listener.accept().await {
            Ok(x) => x,
            Err(err) => {
                warn!("accept failed: {err}");
                continue;
            }
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            if let Err(err) = serve(conn, handler).await {
                warn!("connection ended: {err}");
            }
        });
    }
}

async fn serve(mut conn: UnixStream, handler: Arc<DaemonHandler>) -> std::io::Result<()> {
    loop {
        let req = match Request::recv_async(&mut conn).await {
            Ok(r) => r,
            // Clean EOF when the client closes the socket.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(e),
        };
        let response: Response = handler.handle(req).await?;
        response.send_async(&mut conn).await?;
    }
}
