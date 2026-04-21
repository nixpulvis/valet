//! Browser native messaging shim for the valet addon.
//!
//! Speaks the WebExtensions native messaging wire format on stdin/stdout:
//! each message is a 4-byte little-endian length followed by a UTF-8 JSON
//! payload. This program is a stateless byte pump — it does not know about
//! individual [`Request`]/[`Response`] variants. The browser side encodes
//! a `valetd::Request` with [`Frame::encode_base64`] and posts
//!
//! ```text
//! { "id": <n>, "request": "<base64-bitcode>" }
//! ```
//!
//! The shim base64-decodes the `request` field, writes the raw bitcode bytes
//! to the `valetd` Unix socket as a length-prefixed frame, reads the reply
//! frame, base64-encodes it, and returns
//!
//! ```text
//! { "id": <n>, "result": "<base64-bitcode-Response>" }
//! ```
//!
//! Transport-level failures come back as `{ "id": <n>, "error": "..." }`;
//! application-level errors travel inside the bitcode payload as
//! `Response::Error` and are the browser's problem to decode. Adding a new
//! RPC variant therefore touches only the browser extension and the
//! daemon — never this shim.
//!
//! The shim auto-spawns a sibling `valetd` binary the first time it fails to
//! reach the socket (for example, on first use after install). The sibling
//! is expected to live in the same directory as the shim binary. The daemon
//! detaches and outlives the shim; the idle reaper in `valetd` is what
//! eventually clears cached keys.
//!
//! [`Request`]: valetd::Request
//! [`Response`]: valetd::Response
//! [`Frame::encode_base64`]: valetd::request::Frame::encode_base64

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::Mutex;
use valetd::{
    request::MAX_FRAME_LEN,
    socket,
};

/// Maximum native messaging frame size (1 MiB). The browser enforces a
/// similar limit on the addon side.
const MAX_FRAME_SIZE: usize = 1024 * 1024;

/// How long to wait for the daemon's socket to appear after we spawn it.
const SPAWN_SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
const SPAWN_SOCKET_POLL: Duration = Duration::from_millis(50);

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let socket_path: PathBuf = std::env::var_os("VALET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(socket::default_path);

    let conn = match ensure_daemon(&socket_path).await {
        Ok(c) => c,
        Err(err) => {
            eprintln!("valet-native-host: cannot reach valetd: {err}");
            std::process::exit(1);
        }
    };
    let conn = Mutex::new(conn);

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    loop {
        let mut len_buf = [0u8; 4];
        if stdin.read_exact(&mut len_buf).await.is_err() {
            break;
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > MAX_FRAME_SIZE {
            eprintln!("valet-native-host: invalid frame length {len}");
            break;
        }
        let mut buf = vec![0u8; len];
        if stdin.read_exact(&mut buf).await.is_err() {
            break;
        }

        let response = match serde_json::from_slice::<Value>(&buf) {
            Ok(req) => handle(&conn, req).await,
            Err(e) => json!({ "id": Value::Null, "error": format!("invalid json: {e}") }),
        };

        let bytes = match serde_json::to_vec(&response) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("valet-native-host: failed to serialize response: {e}");
                continue;
            }
        };
        if bytes.len() > MAX_FRAME_SIZE {
            eprintln!(
                "valet-native-host: response too large ({} bytes)",
                bytes.len()
            );
            continue;
        }
        let header = (bytes.len() as u32).to_le_bytes();
        if stdout.write_all(&header).await.is_err() || stdout.write_all(&bytes).await.is_err() {
            break;
        }
        if stdout.flush().await.is_err() {
            break;
        }
    }
}

/// Connect to the daemon, spawning a sibling `valetd` first if the socket is
/// absent. Returns the open stream.
async fn ensure_daemon(socket_path: &Path) -> std::io::Result<UnixStream> {
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
fn spawn_valetd() -> std::io::Result<()> {
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("valetd")))
        .filter(|p| p.exists());
    let program: PathBuf = sibling.unwrap_or_else(|| PathBuf::from("valetd"));

    // Detach: new session, stdio -> /dev/null, so the daemon outlives us.
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
    // Inline the libc call without adding a libc crate dep.
    unsafe extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        setsid();
    }
}

/// Pull the `id` and base64 `request` field out of the envelope, forward
/// the raw bitcode bytes to `valetd`, and wrap the reply bytes back up.
/// The shim never decodes the bitcode.
async fn handle(conn: &Mutex<UnixStream>, req: Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);

    let request_b64 = match req.get("request").and_then(Value::as_str) {
        Some(s) => s,
        None => return json!({ "id": id, "error": "missing 'request' field" }),
    };
    let request_bytes = match STANDARD.decode(request_b64) {
        Ok(b) => b,
        Err(e) => return json!({ "id": id, "error": format!("invalid base64: {e}") }),
    };
    if request_bytes.len() > MAX_FRAME_LEN {
        return json!({ "id": id, "error": "request exceeds MAX_FRAME_LEN" });
    }

    let reply_bytes = {
        let mut stream = conn.lock().await;
        match round_trip_bytes(&mut stream, &request_bytes).await {
            Ok(b) => b,
            Err(e) => return json!({ "id": id, "error": format!("daemon io: {e}") }),
        }
    };

    json!({ "id": id, "result": STANDARD.encode(&reply_bytes) })
}

/// Write one length-prefixed frame of bytes to the daemon socket and read
/// one back. Mirrors `Frame::send` / `Frame::recv` at the byte level so the
/// shim can forward without decoding the bitcode payload.
async fn round_trip_bytes(
    conn: &mut UnixStream,
    payload: &[u8],
) -> std::io::Result<Vec<u8>> {
    let len = u32::try_from(payload.len()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "request too large")
    })?;
    conn.write_all(&len.to_be_bytes()).await?;
    conn.write_all(payload).await?;
    conn.flush().await?;

    let mut len_bytes = [0u8; 4];
    conn.read_exact(&mut len_bytes).await?;
    let reply_len = u32::from_be_bytes(len_bytes) as usize;
    if reply_len > MAX_FRAME_LEN {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "reply exceeds MAX_FRAME_LEN",
        ));
    }
    let mut reply = vec![0u8; reply_len];
    conn.read_exact(&mut reply).await?;
    Ok(reply)
}
