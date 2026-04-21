//! Unix-socket backend: forwards request bytes to a sibling `valetd`.
//!
//! Auto-spawns a detached `valetd` the first time the socket is missing,
//! so a freshly-installed extension works without the user starting a
//! daemon manually. The child outlives this process.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use valetd::{request::MAX_FRAME_LEN, socket};

use super::Backend;

/// How long to wait for the daemon's socket to appear after we spawn it.
const SPAWN_SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const SPAWN_SOCKET_POLL: Duration = Duration::from_millis(50);

pub(crate) struct SocketBackend {
    stream: Mutex<UnixStream>,
}

impl SocketBackend {
    pub(crate) async fn build() -> Result<Self, String> {
        let socket_path: PathBuf = std::env::var_os("VALET_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(socket::default_path);
        let stream = ensure_daemon(&socket_path)
            .await
            .map_err(|e| format!("cannot reach valetd at {}: {e}", socket_path.display()))?;
        Ok(Self {
            stream: Mutex::new(stream),
        })
    }
}

impl Backend for SocketBackend {
    async fn round_trip(&self, request_bytes: &[u8]) -> io::Result<Vec<u8>> {
        let mut stream = self.stream.lock().await;
        round_trip_bytes(&mut *stream, request_bytes).await
    }
}

/// Connect to the daemon, spawning a sibling `valetd` first if the socket
/// is absent. Returns the open stream.
async fn ensure_daemon(socket_path: &Path) -> io::Result<UnixStream> {
    if let Ok(conn) = UnixStream::connect(socket_path).await {
        return Ok(conn);
    }
    spawn_valetd()?;
    // Poll until the daemon binds its socket or we give up.
    let deadline = std::time::Instant::now() + SPAWN_SOCKET_TIMEOUT;
    loop {
        match UnixStream::connect(socket_path).await {
            Ok(conn) => return Ok(conn),
            Err(err) => {
                if std::time::Instant::now() >= deadline {
                    return Err(err);
                }
                tokio::time::sleep(SPAWN_SOCKET_POLL).await;
            }
        }
    }
}

/// Spawn the sibling `valetd` binary. Looks next to the current executable
/// first; falls back to `PATH`.
fn spawn_valetd() -> io::Result<()> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("valetd")))
        .filter(|p| p.exists());
    let program: PathBuf = sibling.unwrap_or_else(|| PathBuf::from("valetd"));

    let mut cmd = std::process::Command::new(program);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    unsafe {
        use std::os::unix::process::CommandExt;
        cmd.pre_exec(|| {
            // Detach from the shim's controlling terminal and process group
            // so a terminating shim does not take the daemon with it.
            libc_setsid();
            Ok(())
        });
    }
    cmd.spawn().map(|_child| ())
}

#[cfg(unix)]
fn libc_setsid() {
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}

/// Write one length-prefixed frame to the daemon socket and read one back.
/// Mirrors `Frame::send` / `Frame::recv` at the byte level so the shim can
/// forward without decoding the bitcode payload.
async fn round_trip_bytes(conn: &mut UnixStream, payload: &[u8]) -> io::Result<Vec<u8>> {
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "request too large"))?;
    conn.write_all(&len.to_be_bytes()).await?;
    conn.write_all(payload).await?;
    conn.flush().await?;

    let mut len_bytes = [0u8; 4];
    conn.read_exact(&mut len_bytes).await?;
    let reply_len = u32::from_be_bytes(len_bytes) as usize;
    if reply_len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "reply exceeds MAX_FRAME_LEN",
        ));
    }
    let mut reply = vec![0u8; reply_len];
    conn.read_exact(&mut reply).await?;
    Ok(reply)
}
