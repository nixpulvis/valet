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
//! Two backends are always compiled in. At startup the shim probes
//! `valetd`'s Unix socket: if something is listening it uses
//! [`SocketBackend`] (pure byte relay), otherwise it falls back to
//! [`EmbeddedBackend`], which owns a [`valetd::DaemonHandler`] directly
//! and is the whole server. `VALET_BACKEND=socket|embedded|auto` forces
//! a specific choice; see [`backend`] for the selection rule and the
//! caveat around the user starting `valetd` after the shim.
//!
//! Adding a new RPC variant touches neither backend — only the browser
//! extension and `valetd` itself.
//!
//! [`Frame::encode_base64`]: valetd::request::Frame::encode_base64
//! [`SocketBackend`]: backend::SocketBackend
//! [`EmbeddedBackend`]: backend::EmbeddedBackend

use base64::{Engine, engine::general_purpose::STANDARD};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, error, warn};
use valet_browser_bridge::{NativePayload, NativeReply, NativeRequest};
use valetd::request::MAX_FRAME_LEN;

mod backend;
use backend::{Active, Backend};

/// Maximum native messaging frame size (1 MiB). The browser enforces a
/// similar limit on the addon side.
const MAX_FRAME_SIZE: usize = 1024 * 1024;

// Multi-thread so the `valet` library's storgit work can use
// `tokio::task::block_in_place` (panics on a current_thread runtime)
// to offload sync git+fs work off the async path.
#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() {
    valet::logging::init();
    let backend = match backend::build().await {
        Ok(b) => b,
        Err(err) => {
            error!("{err}");
            std::process::exit(1);
        }
    };

    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    loop {
        let mut len_buf = [0u8; 4];
        match stdin.read_exact(&mut len_buf).await {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                debug!("stdin closed, exiting");
                break;
            }
            Err(e) => {
                // stdin is in a bad state, won't recover.
                warn!("stdin read failed, exiting: {e}");
                break;
            }
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        if len == 0 || len > MAX_FRAME_SIZE {
            // TODO: framing has no resync marker, so a bad length forces
            // us to drop the stream. Chunked frames with a sync token
            // would let us skip to the next valid boundary instead.
            warn!(len, "invalid frame length, exiting");
            break;
        }
        let mut buf = vec![0u8; len];
        if let Err(e) = stdin.read_exact(&mut buf).await {
            // TODO: same as above.
            warn!(len, "truncated frame body, exiting: {e}");
            break;
        }

        let response = match serde_json::from_slice::<NativeRequest>(&buf) {
            Ok(req) => handle(&backend, req).await,
            Err(e) => {
                let backend_name: &'static str = (&backend).into();
                warn!(backend = backend_name, "invalid json from browser: {e}");
                NativeReply {
                    backend: backend_name.to_owned(),
                    payload: Err(format!("invalid json: {e}")),
                }
            }
        };

        let bytes = match serde_json::to_vec(&response) {
            Ok(b) => b,
            Err(e) => {
                warn!("failed to serialize response: {e}");
                continue;
            }
        };
        if bytes.len() > MAX_FRAME_SIZE {
            warn!(bytes = bytes.len(), "response too large");
            continue;
        }
        // TODO: same as above.
        let header = (bytes.len() as u32).to_le_bytes();
        if let Err(e) = stdout.write_all(&header).await {
            warn!("stdout write failed (header), exiting: {e}");
            break;
        }
        if let Err(e) = stdout.write_all(&bytes).await {
            warn!("stdout write failed (body), exiting: {e}");
            break;
        }
        if let Err(e) = stdout.flush().await {
            warn!("stdout flush failed, exiting: {e}");
            break;
        }
    }
}

/// Forward the bitcode request to the backend and build the reply
/// envelope. This function never decodes the bitcode — that's the
/// backend's job (and only the embedded backend does it).
async fn handle(backend: &Active, req: NativeRequest) -> NativeReply {
    let backend_name: &'static str = backend.into();

    let request_bytes = match STANDARD.decode(&req.request) {
        Ok(b) => b,
        Err(e) => return error(backend, format!("invalid base64: {e}")),
    };
    if request_bytes.len() > MAX_FRAME_LEN {
        return error(backend, "request exceeds MAX_FRAME_LEN".into());
    }

    match backend.round_trip(&request_bytes).await {
        Ok(reply_bytes) => NativeReply {
            backend: backend_name.to_owned(),
            payload: Ok(NativePayload {
                id: req.id,
                data: STANDARD.encode(&reply_bytes),
            }),
        },
        Err(e) => error(backend, format!("backend: {e}")),
    }
}

/// Log the error and wrap it as a failed [`NativeReply`]. One source
/// of truth for the message.
fn error(backend: &Active, msg: String) -> NativeReply {
    let backend_name: &'static str = backend.into();
    tracing::error!(backend = backend_name, "{msg}");
    NativeReply {
        backend: backend_name.to_owned(),
        payload: Err(msg),
    }
}
