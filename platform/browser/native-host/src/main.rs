//! Browser native messaging shim for the valet addon.
//!
//! Speaks the WebExtensions native messaging wire format on stdin/stdout:
//! each message is a 4-byte little-endian length followed by a UTF-8 JSON
//! payload. The browser side encodes a `valetd::Request` with
//! [`Frame::encode_base64`] and posts
//!
//! ```text
//! { "id": <n>, "request": "<base64-bitcode>" }
//! ```
//!
//! The shim hands the request bytes to a [`Backend`] and wraps the reply
//! bytes as
//!
//! ```text
//! { "id": <n>, "result": "<base64-bitcode-Response>" }
//! ```
//!
//! Transport-level failures come back as `{ "id": <n>, "error": "..." }`;
//! application-level errors travel inside the bitcode payload as
//! `Response::Error`.
//!
//! Two backends, selected at compile time:
//!
//! * [`SocketBackend`] (default) — forwards the raw bitcode bytes to a
//!   running `valetd` over its Unix socket, auto-spawning a sibling
//!   daemon if the socket is missing. The shim never decodes the payload.
//! * [`EmbeddedBackend`] (`--features embedded`) — owns a
//!   [`valetd::DaemonHandler`] in-process, so the shim is the whole
//!   server. Pulls in SQLite and crypto; no socket is involved.
//!
//! Adding a new RPC variant touches neither backend — only the browser
//! extension and `valetd` itself.
//!
//! [`Frame::encode_base64`]: valetd::request::Frame::encode_base64
//! [`SocketBackend`]: backend::SocketBackend
//! [`EmbeddedBackend`]: backend::EmbeddedBackend

use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use valetd::request::MAX_FRAME_LEN;

mod backend;
use backend::Backend;

/// Maximum native messaging frame size (1 MiB). The browser enforces a
/// similar limit on the addon side.
const MAX_FRAME_SIZE: usize = 1024 * 1024;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let backend = match backend::build().await {
        Ok(b) => b,
        Err(err) => {
            eprintln!("valet-native-host: {err}");
            std::process::exit(1);
        }
    };

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
            Ok(req) => handle(&backend, req).await,
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

/// Pull `id` and base64 `request` from the envelope, hand the request
/// bytes to the backend, and wrap the reply bytes back up. This function
/// never decodes the bitcode — that's the backend's job (and only the
/// embedded backend does it).
async fn handle<B: Backend>(backend: &B, req: Value) -> Value {
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

    match backend.round_trip(&request_bytes).await {
        Ok(reply_bytes) => json!({ "id": id, "result": STANDARD.encode(&reply_bytes) }),
        Err(e) => json!({ "id": id, "error": format!("backend: {e}") }),
    }
}
