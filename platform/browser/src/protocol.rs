//! Native-messaging client bridge used by the popup and content
//! scripts.
//!
//! Both sides build typed [`Call`] payloads and issue them through
//! [`NativeMessageClient`], a [`Handler`] that frames each request as
//! a base64 bitcode string and forwards it via
//! `browser.runtime.sendMessage` to the background script. The
//! background pumps the bytes to the native host over its shared
//! [`valet::protocol::NativeMessageClient`].
//!
//! Typical use:
//!
//! ```ignore
//! let list = protocol::NativeMessageClient.call(ListUsers).await?;
//! ```
//!
//! [`Call`]: valet::protocol::message::Call
//! [`Handler`]: valet::Handler

use serde::Deserialize;
use std::io;
use valet::protocol::frame::Frame;
use valet::{Handler, Request, Response};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

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

/// Unit [`Handler`] that routes every [`Request`] through
/// `browser.runtime.sendMessage` and decodes the base64 [`Response`]
/// carried back. Use [`Handler::call`] with any typed payload
/// struct.
///
/// [`Handler`]: valet::Handler
/// [`Handler::call`]: valet::Handler::call
pub struct NativeMessageClient;

impl Handler for NativeMessageClient {
    async fn handle(&self, request: Request) -> io::Result<Response> {
        let kind: &'static str = (&request).into();
        let request_b64 = request.encode_base64();
        tracing::trace!(request = kind, "-> rpc request");
        let promise = runtime_send_message(JsValue::from_str(&request_b64))
            .map_err(|v| io::Error::other(format!("send: {}", format_js_value(&v))))?;
        let js_result = JsFuture::from(promise)
            .await
            .map_err(|v| io::Error::other(format!("send: {}", format_js_value(&v))))?;
        let reply: Reply = serde_wasm_bindgen::from_value(js_result)
            .map_err(|e| io::Error::other(format!("envelope: {e}")))?;
        let response = Response::decode_base64(&reply.result).map_err(|e| {
            tracing::trace!("({}) <- rpc reply decode error", reply.backend);
            io::Error::other(format!("decode: {e}"))
        })?;
        let resp_kind: &'static str = (&response).into();
        tracing::trace!(response = resp_kind, "({}) <- rpc reply", reply.backend);
        Ok(response)
    }
}

/// Return the first currently-unlocked username, or `None` if no user
/// is unlocked.
pub async fn first_unlocked_user() -> Result<Option<String>, valet::protocol::Error> {
    Ok(NativeMessageClient
        .call(valet::protocol::message::Status)
        .await?
        .into_iter()
        .next())
}
