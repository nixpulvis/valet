//! Shared RPC transport between the popup and content scripts.
//!
//! Both sides build a [`valet::Request`] directly and hand it to
//! [`call`]; the background script is a transparent byte pump to the
//! native host. Each call sends the base64-encoded bitcode [`Request`]
//! as the `runtime.sendMessage` payload and gets back
//!
//! ```text
//! { "result": "<base64-bitcode-Response>", "backend": "socket" | "embedded" }
//! ```
//!
//! Callers unpack the expected [`Response`] variant with the
//! [`Response::expect_ok`], [`Response::expect_users`],
//! [`Response::expect_index`], and [`Response::expect_record`] helpers
//! defined in `valet::protocol::message`. Those return [`ResponseError`],
//! which bubbles through `?` into the [`Error`] used here.

use serde::Deserialize;
use valet::{
    Request, Response,
    protocol::frame::{DecodeError, Frame},
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

/// Transport-layer failure reaching the background or decoding its reply.
/// Wrapped in [`valet::protocol::message::Error::Rpc`] by [`call`] so daemon-reported
/// errors and unexpected variants flow through the same `?` chain.
#[derive(Debug)]
pub enum Error {
    /// `runtime.sendMessage` rejected (no background listener, etc).
    Send(JsValue),
    /// Failed to (de)serialize the runtime-message envelope.
    Envelope(serde_wasm_bindgen::Error),
    /// Failed to decode base64 or bitcode in the reply payload.
    Decode(DecodeError),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Send(v) => write!(f, "send: {}", format_js_value(v)),
            Error::Envelope(e) => write!(f, "envelope: {e}"),
            Error::Decode(e) => write!(f, "decode: {e}"),
        }
    }
}

fn format_js_value(v: &JsValue) -> String {
    if let Some(s) = v.as_string() {
        return s;
    }
    if let Ok(msg) = js_sys::Reflect::get(v, &JsValue::from_str("message"))
        && let Some(s) = msg.as_string()
    {
        return s;
    }
    format!("{v:?}")
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["browser", "runtime"], js_name = sendMessage, catch)]
    fn runtime_send_message(msg: JsValue) -> Result<js_sys::Promise, JsValue>;
}

#[derive(Deserialize)]
struct Reply {
    result: String,
    backend: String,
}

/// Send a [`Request`] to the background and decode the base64 reply
/// into a [`Response`]. The backend tag (`"socket"` or `"embedded"`) is
/// logged at trace so it's visible alongside each RPC without threading
/// through every call site.
pub async fn call(request: Request) -> Result<Response, valet::protocol::message::Error<Error>> {
    use valet::protocol::message::Error as OuterError;
    let kind: &'static str = (&request).into();
    let request_b64 = request.encode_base64();
    tracing::trace!(request = kind, "→ rpc request");
    let promise = runtime_send_message(JsValue::from_str(&request_b64))
        .map_err(|v| OuterError::Rpc(Error::Send(v)))?;
    let js_result = JsFuture::from(promise)
        .await
        .map_err(|v| OuterError::Rpc(Error::Send(v)))?;
    let reply: Reply = serde_wasm_bindgen::from_value(js_result)
        .map_err(|e| OuterError::Rpc(Error::Envelope(e)))?;
    let response = Response::decode_base64(&reply.result).map_err(|e| {
        tracing::trace!("({}) ← rpc reply decode error", reply.backend);
        OuterError::Rpc(Error::Decode(e))
    })?;
    let kind: &'static str = (&response).into();
    tracing::trace!(response = kind, "({}) ← rpc reply", reply.backend);
    Ok(response)
}

/// Return the first currently-unlocked username, or `None` if no user
/// is unlocked. The daemon's [`Request::Status`] handler always replies
/// with [`Response::Users`], so the only errors surfaced here are
/// transport-level failures from [`call`]; a `Response` path would
/// indicate the handler contract changed.
pub async fn first_unlocked_user() -> Result<Option<String>, valet::protocol::message::Error<Error>>
{
    Ok(call(Request::Status)
        .await?
        .expect_users()?
        .into_iter()
        .next())
}
