//! `valetd` - the Valet daemon.
//!
//! Owns a single SQLite database, a cache of unlocked [`User`] / [`Lot`]
//! keys, and serves the [`valetd::Request`] / [`valetd::Response`] protocol
//! on a Unix socket. Cached keys are dropped after [`IDLE_TIMEOUT`] with no
//! activity; they are also dropped when the process exits because
//! [`valet::encrypt::Key`] is `ZeroizeOnDrop`.
//!
//! All request handling lives in [`valetd::server::DaemonHandler`]; this
//! binary is the Unix-socket transport and the idle reaper around it.
//!
//! Socket path: `$VALET_SOCKET` if set, otherwise [`valetd::socket::default_path`].
//! Database path: `$VALET_DB` if set, otherwise [`valet::db::default_url`].
//!
//! [`User`]: valet::user::User
//! [`Lot`]: valet::Lot

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use valetd::{
    DaemonHandler, Handler, Request, Response,
    request::Frame,
    socket,
};

const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(15);

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let socket_path: PathBuf = std::env::var_os("VALET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(socket::default_path);

    let handler = match DaemonHandler::from_env().await {
        Ok(h) => h,
        Err(err) => {
            eprintln!("valetd: {err}");
            std::process::exit(1);
        }
    };

    if let Some(parent) = socket_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            eprintln!(
                "valetd: failed to create socket directory {}: {err}",
                parent.display()
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
            eprintln!("valetd: failed to bind {}: {err}", socket_path.display());
            std::process::exit(1);
        }
    };
    eprintln!("valetd: listening on {}", socket_path.display());

    let handler = Arc::new(handler);

    // Background reaper: drops cached keys after IDLE_TIMEOUT of inactivity.
    {
        let handler = handler.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(IDLE_CHECK_INTERVAL).await;
                if handler.reap_if_idle(IDLE_TIMEOUT).await {
                    eprintln!("valetd: idle timeout, locked all users");
                }
            }
        });
    }

    loop {
        let (conn, _) = match listener.accept().await {
            Ok(x) => x,
            Err(err) => {
                eprintln!("valetd: accept failed: {err}");
                continue;
            }
        };
        let handler = handler.clone();
        tokio::spawn(async move {
            if let Err(err) = serve(conn, handler).await {
                eprintln!("valetd: connection ended: {err}");
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
