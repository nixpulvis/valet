//! Native-host port management and the popup/content-facing message listener.
//!
//! Wasm is single-threaded, but `thread_local!` is used here as the
//! ergonomic way to hold mutable `'static` state in safe Rust (a plain
//! `static` would require `Sync`, and `static mut` is `unsafe`).  On a
//! single-threaded target it compiles to plain static access with no
//! overhead.  The thread-locals are: one cached [`Port`], a `pending`
//! map keyed by RPC id, and a monotonic id counter.
//!
//! The background is a transparent byte pump: every
//! `runtime.sendMessage` payload is a base64-encoded [`valetd::Request`]
//! that gets forwarded to the native host inside an `{ id, request }`
//! envelope, and the base64 [`valetd::Response`] comes back as
//! `{ result, backend }`. Adding an RPC only touches the popup/content
//! wrappers and `valetd` — the background is agnostic to message types.

use std::cell::RefCell;
use std::collections::HashMap;

use futures::channel::oneshot;
use serde::Serialize;
use valet_browser_bridge::{NativeReply, NativeRequest};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use super::externs::{self, Port};

const NATIVE_APP: &str = "com.nixpulvis.valet";

/// Two layers of failure are possible and we want to distinguish
/// them. The outer `Err` means no reply envelope arrived at all:
/// port closed, or the bytes we got could not be deserialised into a
/// [`NativeReply`]. There is no backend tag in that case because we
/// don't know which side (or neither) was actually reached. The
/// inner `Err` (inside [`NativeReply::payload`]) means a well-formed
/// reply arrived from a known backend but the request itself failed
/// application-side, so callers can log with `backend = ...`.
type NativeResult = Result<NativeReply, String>;

thread_local! {
    static PORT: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static PENDING: RefCell<HashMap<u32, oneshot::Sender<NativeResult>>> =
        RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u32> = const { RefCell::new(1) };
}

/// Register the `browser.runtime.onMessage` handler that forwards RPC
/// calls from the popup or content scripts to the native host and
/// returns the base64 reply plus a backend tag.
pub fn install_message_listener() {
    let cb = Closure::wrap(Box::new(|msg: JsValue, _sender: JsValue| -> JsValue {
        future_to_promise(async move { handle_rpc(msg).await }).into()
    }) as Box<dyn FnMut(JsValue, JsValue) -> JsValue>);
    externs::runtime_on_message_add_listener(cb.as_ref().unchecked_ref());
    cb.forget();
}

/// Forward a raw RPC call to the native host. The caller has already
/// encoded its [`valetd::Request`] to base64; the background never
/// decodes it. The return shape is `{ result: <b64>, backend: <tag> }`
/// so callers can log which transport served the call.
async fn handle_rpc(msg: JsValue) -> Result<JsValue, JsValue> {
    let request_b64 = msg
        .as_string()
        .ok_or_else(|| JsValue::from_str("message must be a base64 string"))?;
    let reply = match call_native(&request_b64).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "rpc transport failed");
            return Err(JsValue::from_str(&e));
        }
    };
    match reply.payload {
        Ok(p) => {
            tracing::trace!(id = p.id, "({}) rpc ok", reply.backend);
            #[derive(Serialize)]
            struct Resp<'a> {
                result: &'a str,
                backend: &'a str,
            }
            let serializer = serde_wasm_bindgen::Serializer::json_compatible();
            serde::Serialize::serialize(
                &Resp {
                    result: &p.data,
                    backend: &reply.backend,
                },
                &serializer,
            )
            .map_err(|e| JsValue::from_str(&e.to_string()))
        }
        Err(e) => {
            tracing::error!(error = %e, "({}) rpc error", reply.backend);
            Err(JsValue::from_str(&e))
        }
    }
}

/// Send an already-base64-encoded request to the native host and return
/// the reply (backend tag + base64 result or error message).
async fn call_native(request_b64: &str) -> NativeResult {
    let id = NEXT_ID.with(|n| {
        let mut n = n.borrow_mut();
        let id = *n;
        *n = n.wrapping_add(1).max(1);
        id
    });
    let (tx, rx) = oneshot::channel();
    PENDING.with(|p| p.borrow_mut().insert(id, tx));

    let envelope = NativeRequest {
        id,
        request: request_b64.to_owned(),
    };
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    let frame_js =
        serde::Serialize::serialize(&envelope, &serializer).map_err(|e| e.to_string())?;

    tracing::trace!(id, "→ native host");
    ensure_port().post_message(&frame_js);

    let result = rx.await.map_err(|_| "native port closed".to_string())?;
    match &result {
        Ok(r) => match &r.payload {
            Ok(p) => tracing::trace!(id = p.id, "({}) ← native host", r.backend),
            Err(e) => tracing::error!(error = %e, "({}) ← native host error", r.backend),
        },
        Err(e) => tracing::error!(id, error = %e, "← native host transport error"),
    }
    result
}

fn ensure_port() -> Port {
    PORT.with(|cell| {
        if let Some(v) = cell.borrow().as_ref() {
            return v.clone().unchecked_into::<Port>();
        }
        tracing::info!(app = NATIVE_APP, "connecting native host");
        let port = externs::connect_native(NATIVE_APP);
        attach_handlers(&port);
        let value: JsValue = port.into();
        *cell.borrow_mut() = Some(value.clone());
        value.unchecked_into::<Port>()
    })
}

fn attach_handlers(port: &Port) {
    let on_msg = Closure::wrap(Box::new(|msg: JsValue| {
        let reply: NativeReply = match serde_wasm_bindgen::from_value(msg) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("native reply malformed: {e}");
                return;
            }
        };
        // Only success payloads carry an id, so native-host errors
        // where no id was extractable (e.g. malformed request JSON)
        // cannot be routed to a pending sender; log and drop.
        let id = match &reply.payload {
            Ok(p) => p.id,
            Err(e) => {
                tracing::error!(backend = %reply.backend, error = %e, "native reply without id");
                return;
            }
        };
        let sender = PENDING.with(|p| p.borrow_mut().remove(&id));
        let Some(sender) = sender else {
            tracing::error!(id, backend = %reply.backend, "native reply for unknown id");
            return;
        };
        let _ = sender.send(Ok(reply));
    }) as Box<dyn FnMut(JsValue)>);
    port.on_message()
        .add_listener(on_msg.as_ref().unchecked_ref());
    on_msg.forget();

    let on_disc = Closure::wrap(Box::new(|_port: JsValue| {
        tracing::warn!("native port disconnected");
        PORT.with(|cell| *cell.borrow_mut() = None);
        PENDING.with(|p| {
            for (_id, sender) in p.borrow_mut().drain() {
                let _ = sender.send(Err("native host disconnected".into()));
            }
        });
    }) as Box<dyn FnMut(JsValue)>);
    port.on_disconnect()
        .add_listener(on_disc.as_ref().unchecked_ref());
    on_disc.forget();
}
