//! Helpers for driving `Server<NativeMessage>` through a pair of
//! `tokio::io::duplex` pipes. Used by `tests/protocol/native_msg.rs`
//! (envelope-level tests) and `tests/protocol/multi.rs` (relay
//! composition tests); colocated here so both stay in sync.

use base64::{Engine, engine::general_purpose::STANDARD};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use valet::Handler;
use valet::protocol::frame::Frame;
use valet::protocol::message::{Request, Response};
use valet::protocol::native_msg::{self, NativeId, NativeReply, NativeRequest};

/// Encode and write one native-messaging request envelope to
/// `to_server` (the pipe the server is reading).
pub async fn send(to_server: &mut tokio::io::DuplexStream, id: NativeId, request: &Request) {
    let env = NativeRequest {
        id,
        request: STANDARD.encode(request.encode()),
    };
    let body = serde_json::to_vec(&env).unwrap();
    write_frame(to_server, &body).await;
}

/// Write a raw body as a framed envelope. Useful for sending
/// malformed JSON or crafted bytes the server must tolerate.
pub async fn write_frame(to_server: &mut tokio::io::DuplexStream, body: &[u8]) {
    let header = (body.len() as u32).to_le_bytes();
    to_server.write_all(&header).await.unwrap();
    to_server.write_all(body).await.unwrap();
    to_server.flush().await.unwrap();
}

/// Read one reply envelope off `from_server` (the pipe the server
/// is writing to).
pub async fn recv(from_server: &mut tokio::io::DuplexStream) -> NativeReply {
    let mut len_buf = [0u8; 4];
    from_server.read_exact(&mut len_buf).await.unwrap();
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    from_server.read_exact(&mut buf).await.unwrap();
    serde_json::from_slice(&buf).unwrap()
}

/// Extract the decoded bitcode `Response` from a reply's success
/// payload. Panics on an error reply - use `recv` directly when you
/// expect an error envelope.
pub fn payload_response(reply: &NativeReply) -> Response {
    let payload = reply
        .payload
        .as_ref()
        .unwrap_or_else(|e| panic!("expected Ok payload, got Err({e})"));
    let bytes = STANDARD.decode(&payload.data).unwrap();
    Response::decode(&bytes).unwrap()
}

/// Stand up `native_msg::serve_io` on a pair of duplex pipes,
/// returning the two halves (`to_server`, `from_server`) the test
/// uses to act as the browser, plus the server task handle.
pub fn spawn_server<H: Handler + 'static>(
    backend: &'static str,
    handler: Arc<H>,
) -> (
    tokio::io::DuplexStream,
    tokio::io::DuplexStream,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let (to_server, server_in) = tokio::io::duplex(64 * 1024);
    let (server_out, from_server) = tokio::io::duplex(64 * 1024);
    let task =
        tokio::spawn(
            async move { native_msg::serve_io(backend, server_in, server_out, handler).await },
        );
    (to_server, from_server, task)
}
