//! Native-host port management and the popup-facing message listener.
//!
//! Wasm is single-threaded, but `thread_local!` is used here as the
//! ergonomic way to hold mutable `'static` state in safe Rust (a plain
//! `static` would require `Sync`, and `static mut` is `unsafe`).  On a
//! single-threaded target it compiles to plain static access with no
//! overhead.  The thread-locals are: one cached [`Port`], a `pending`
//! map keyed by RPC id, and a monotonic id counter.
//!
//! The background script acts as a transparent message broker: it forwards
//! RPC requests to the native host and passes the raw base64-encoded result
//! string back to the popup without decoding it.

use std::cell::RefCell;
use std::collections::HashMap;

use futures::channel::oneshot;
use js_sys::Reflect;
use serde::Serialize;
use valet::lot::DEFAULT_LOT;
use valetd::{Response, request::Frame};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use super::externs::{self, Port};

const NATIVE_APP: &str = "com.nixpulvis.valet";

/// The raw response from the native host: either a base64-encoded bitcode
/// blob (success) or an error string.
pub(crate) type NativeResult = Result<String, String>;

thread_local! {
    static PORT: RefCell<Option<JsValue>> = const { RefCell::new(None) };
    static PENDING: RefCell<HashMap<u32, oneshot::Sender<NativeResult>>> =
        RefCell::new(HashMap::new());
    static NEXT_ID: RefCell<u32> = const { RefCell::new(1) };
}

/// Register the `browser.runtime.onMessage` handler that forwards popup
/// RPC calls to the native host and returns the response.
pub fn install_message_listener() {
    let cb = Closure::wrap(Box::new(|msg: JsValue, _sender: JsValue| -> JsValue {
        future_to_promise(async move { handle_message(msg).await }).into()
    }) as Box<dyn FnMut(JsValue, JsValue) -> JsValue>);
    externs::runtime_on_message_add_listener(cb.as_ref().unchecked_ref());
    cb.forget();
}

async fn handle_message(msg: JsValue) -> Result<JsValue, JsValue> {
    let kind = Reflect::get(&msg, &JsValue::from_str("kind"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("missing kind"))?;
    tracing::trace!(kind = %kind, "message received");
    match kind.as_str() {
        "rpc" => handle_rpc(msg).await,
        "autofill_status" => handle_autofill_status().await,
        "autofill_request" => handle_autofill_request(msg).await,
        "autofill_fill" => handle_autofill_fill(msg).await,
        "autofill_generate" => handle_autofill_generate(msg).await,
        other => {
            tracing::warn!(kind = %other, "unknown message kind");
            Err(JsValue::from_str(&format!("unknown message kind: {other}")))
        }
    }
}

/// Forward a raw RPC call from the popup to the native host.
async fn handle_rpc(msg: JsValue) -> Result<JsValue, JsValue> {
    let method = Reflect::get(&msg, &JsValue::from_str("method"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("missing method"))?;
    let params = Reflect::get(&msg, &JsValue::from_str("params"))?;
    match call_native(&method, params).await {
        Ok(b64) => {
            tracing::trace!(method = %method, "rpc ok");
            Ok(JsValue::from_str(&b64))
        }
        Err(e) => {
            tracing::trace!(method = %method, error = %e, "rpc failed");
            Err(JsValue::from_str(&e))
        }
    }
}

/// Return the first currently-unlocked username (if any).
async fn handle_autofill_status() -> Result<JsValue, JsValue> {
    let username = first_unlocked_user().await.ok();
    #[derive(serde::Serialize)]
    struct Resp {
        username: Option<String>,
    }
    to_js(&Resp { username })
}

/// Find matching credentials for a domain. Uses the first unlocked user
/// and the default lot.
async fn handle_autofill_request(msg: JsValue) -> Result<JsValue, JsValue> {
    let domain = Reflect::get(&msg, &JsValue::from_str("domain"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("missing domain"))?;

    let username = match first_unlocked_user().await {
        Ok(u) => u,
        Err(_) => return autofill_error("not unlocked"),
    };

    let result = call_native_rpc(
        "find_records",
        serde_json::json!({
            "username": username,
            "lot": DEFAULT_LOT,
            "domain": domain,
        }),
    )
    .await?;
    let entries = match result {
        Response::Index(entries) => entries,
        other => return autofill_error(&format!("unexpected response: {other:?}")),
    };

    #[derive(serde::Serialize)]
    struct Cred {
        label: String,
        record_uuid: String,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        credentials: Vec<Cred>,
    }
    let credentials = entries
        .iter()
        .map(|(uuid, label)| Cred {
            label: label.to_string(),
            record_uuid: uuid.to_string(),
        })
        .collect();
    to_js(&Resp { credentials })
}

/// Decrypt and return a specific credential for form filling.
async fn handle_autofill_fill(msg: JsValue) -> Result<JsValue, JsValue> {
    let record_uuid = Reflect::get(&msg, &JsValue::from_str("record_uuid"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("missing record_uuid"))?;

    let username = match first_unlocked_user().await {
        Ok(u) => u,
        Err(_) => return autofill_error("not unlocked"),
    };

    let result = call_native_rpc(
        "get_record",
        serde_json::json!({
            "username": username,
            "lot": DEFAULT_LOT,
            "record_uuid": record_uuid,
        }),
    )
    .await?;
    let record = match result {
        Response::Record(r) => r,
        other => return autofill_error(&format!("unexpected response: {other:?}")),
    };

    autofill_credential_response(&record)
}

/// Generate a password, save it as a record, and return the credential.
async fn handle_autofill_generate(msg: JsValue) -> Result<JsValue, JsValue> {
    let label = Reflect::get(&msg, &JsValue::from_str("label"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("missing label"))?;

    let username = match first_unlocked_user().await {
        Ok(u) => u,
        Err(_) => return autofill_error("not unlocked"),
    };

    let result = call_native_rpc(
        "generate_record",
        serde_json::json!({
            "username": username,
            "lot": DEFAULT_LOT,
            "label": label,
        }),
    )
    .await?;
    let record = match result {
        Response::Record(r) => r,
        other => return autofill_error(&format!("unexpected response: {other:?}")),
    };

    autofill_credential_response(&record)
}

/// Build the JSON response for autofill fill/generate, extracting the
/// username from the record's label and the password from its data.
fn autofill_credential_response(record: &valet::Record) -> Result<JsValue, JsValue> {
    #[derive(serde::Serialize)]
    struct Resp {
        username: Option<String>,
        password: String,
    }
    to_js(&Resp {
        username: record.label().username().map(str::to_owned),
        password: record.password().as_str().to_string(),
    })
}

/// Serialize a value to a JSON-compatible `JsValue`.
fn to_js(value: &impl serde::Serialize) -> Result<JsValue, JsValue> {
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    serde::Serialize::serialize(value, &serializer).map_err(|e| JsValue::from_str(&e.to_string()))
}

fn autofill_error(msg: &str) -> Result<JsValue, JsValue> {
    #[derive(serde::Serialize)]
    struct Resp {
        error: String,
    }
    to_js(&Resp {
        error: msg.to_string(),
    })
}

/// Call `status` on the native host and return the first unlocked username.
async fn first_unlocked_user() -> Result<String, JsValue> {
    let b64 = call_native("status", JsValue::from_str("{}"))
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    let result = Response::decode_base64(&b64).map_err(|e| JsValue::from_str(&e.to_string()))?;
    match result {
        Response::Users(users) => users
            .into_iter()
            .next()
            .ok_or_else(|| JsValue::from_str("not unlocked")),
        _ => Err(JsValue::from_str("not unlocked")),
    }
}

/// JSON envelope posted to the native host over the [`Port`](super::externs::Port).
/// The popup's `BrowserEnvelope` carries the same `method` and `params`
/// payload but a different third field: it tags messages with `kind` so
/// the background can route among handlers, whereas this envelope carries
/// an `id` so replies from the native host can be matched back to their
/// pending [`oneshot`] channel in `PENDING`.
#[derive(Serialize)]
pub(crate) struct NativeEnvelope<'a> {
    id: u32,
    method: &'a str,
    params: serde_json::Value,
}

/// Send an RPC call to the native host and return the raw base64 result string.
pub(crate) async fn call_native(method: &str, params: JsValue) -> Result<String, String> {
    let id = NEXT_ID.with(|n| {
        let mut n = n.borrow_mut();
        let id = *n;
        *n = n.wrapping_add(1).max(1);
        id
    });
    let (tx, rx) = oneshot::channel();
    PENDING.with(|p| p.borrow_mut().insert(id, tx));

    let params_json: serde_json::Value =
        serde_wasm_bindgen::from_value(params).map_err(|e| e.to_string())?;
    let envelope = NativeEnvelope {
        id,
        method,
        params: params_json,
    };
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    let frame_js =
        serde::Serialize::serialize(&envelope, &serializer).map_err(|e| e.to_string())?;

    tracing::trace!(id, method, "→ native host");
    ensure_port().post_message(&frame_js);

    let result = rx.await.map_err(|_| "native port closed".to_string())?;
    tracing::trace!(id, method, ok = result.is_ok(), "← native host");
    result
}

/// Call the native host and decode the base64 response into a [`Response`].
///
/// Combines [`call_native`] with result decoding, mapping all errors to
/// [`JsValue`] for use in the autofill message handlers.
async fn call_native_rpc(method: &str, params: serde_json::Value) -> Result<Response, JsValue> {
    let params_js =
        serde_wasm_bindgen::to_value(&params).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let b64 = call_native(method, params_js)
        .await
        .map_err(|e| JsValue::from_str(&e))?;
    Response::decode_base64(&b64).map_err(|e| JsValue::from_str(&e.to_string()))
}

pub(crate) fn ensure_port() -> Port {
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

pub(crate) fn attach_handlers(port: &Port) {
    let on_msg = Closure::wrap(Box::new(|msg: JsValue| {
        let id = Reflect::get(&msg, &JsValue::from_str("id"))
            .ok()
            .and_then(|v| v.as_f64())
            .map(|f| f as u32);
        let Some(id) = id else {
            tracing::warn!("native message missing id");
            return;
        };
        let sender = PENDING.with(|p| p.borrow_mut().remove(&id));
        let Some(sender) = sender else {
            tracing::warn!(id, "native reply for unknown id");
            return;
        };

        let error = Reflect::get(&msg, &JsValue::from_str("error"))
            .ok()
            .and_then(|v| v.as_string());
        if let Some(e) = error {
            tracing::trace!(id, error = %e, "native ← error");
            let _ = sender.send(Err(e));
            return;
        }
        let result_b64 = Reflect::get(&msg, &JsValue::from_str("result"))
            .ok()
            .and_then(|v| v.as_string());
        let Some(b64) = result_b64 else {
            tracing::warn!(id, "native reply missing result");
            let _ = sender.send(Err("missing result field".into()));
            return;
        };
        tracing::trace!(id, bytes = b64.len(), "native ← result");
        let _ = sender.send(Ok(b64));
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
