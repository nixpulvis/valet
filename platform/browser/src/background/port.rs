//! Native-host port management and the popup-facing message listener.
//!
//! Wasm is single-threaded, but `thread_local!` is used here as the
//! ergonomic way to hold mutable `'static` state in safe Rust (a plain
//! `static` would require `Sync`, and `static mut` is `unsafe`).  On a
//! single-threaded target it compiles to plain static access with no
//! overhead.  The thread-locals are: one cached [`Port`], a `pending`
//! map keyed by RPC id, and a monotonic id counter.
//!
//! The background script acts as a transparent byte pump for popup RPCs:
//! it forwards the popup's already-encoded base64 [`valetd::Request`] to
//! the native host in an `{ id, request }` envelope and returns the raw
//! base64 [`valetd::Response`] string back to the popup without decoding
//! it. Adding an RPC only touches the popup wrappers and `valetd` — the
//! background is agnostic to message types.

use std::cell::RefCell;
use std::collections::HashMap;

use futures::channel::oneshot;
use js_sys::Reflect;
use serde::Serialize;
use valet::lot::DEFAULT_LOT;
use valetd::{Request, Response, request::Frame};
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

/// Forward a raw RPC call from the popup to the native host. The popup
/// has already encoded its [`Request`] to base64; the background never
/// decodes it.
async fn handle_rpc(msg: JsValue) -> Result<JsValue, JsValue> {
    let request_b64 = Reflect::get(&msg, &JsValue::from_str("request"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("missing request"))?;
    match call_native_raw(&request_b64).await {
        Ok(b64) => {
            tracing::trace!("rpc ok");
            Ok(JsValue::from_str(&b64))
        }
        Err(e) => {
            tracing::trace!(error = %e, "rpc failed");
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

    let result = call_native_request(Request::FindRecords {
        username,
        lot: DEFAULT_LOT.to_owned(),
        query: domain,
    })
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

    let uuid: valet::uuid::Uuid<valet::Record> = match valet::uuid::Uuid::parse(&record_uuid) {
        Ok(u) => u,
        Err(e) => return autofill_error(&format!("invalid uuid: {e:?}")),
    };
    let result = call_native_request(Request::GetRecord {
        username,
        lot: DEFAULT_LOT.to_owned(),
        uuid,
    })
    .await?;
    let record = match result {
        Response::Record(r) => r,
        other => return autofill_error(&format!("unexpected response: {other:?}")),
    };

    autofill_credential_response(&record)
}

/// Generate a password, save it as a record, and return the credential.
async fn handle_autofill_generate(msg: JsValue) -> Result<JsValue, JsValue> {
    use std::str::FromStr;
    let label = Reflect::get(&msg, &JsValue::from_str("label"))?
        .as_string()
        .ok_or_else(|| JsValue::from_str("missing label"))?;

    let username = match first_unlocked_user().await {
        Ok(u) => u,
        Err(_) => return autofill_error("not unlocked"),
    };

    let label = match valet::record::Label::from_str(&label) {
        Ok(l) => l,
        Err(e) => return autofill_error(&format!("invalid label: {e:?}")),
    };
    let result = call_native_request(Request::GenerateRecord {
        username,
        lot: DEFAULT_LOT.to_owned(),
        label,
    })
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

/// Call `Request::Status` and return the first unlocked username.
async fn first_unlocked_user() -> Result<String, JsValue> {
    let response = call_native_request(Request::Status)
        .await
        .map_err(|e| e)?;
    match response {
        Response::Users(users) => users
            .into_iter()
            .next()
            .ok_or_else(|| JsValue::from_str("not unlocked")),
        _ => Err(JsValue::from_str("not unlocked")),
    }
}

/// JSON envelope posted to the native host over the [`Port`](super::externs::Port).
/// `request` is the base64-encoded bitcode [`valetd::Request`]; the
/// background never inspects the inner variant. `id` lets replies from the
/// native host be matched back to their pending [`oneshot`] channel in
/// `PENDING`.
#[derive(Serialize)]
pub(crate) struct NativeEnvelope<'a> {
    id: u32,
    request: &'a str,
}

/// Send an already-base64-encoded request to the native host and return
/// the raw base64 result string.
pub(crate) async fn call_native_raw(request_b64: &str) -> Result<String, String> {
    let id = NEXT_ID.with(|n| {
        let mut n = n.borrow_mut();
        let id = *n;
        *n = n.wrapping_add(1).max(1);
        id
    });
    let (tx, rx) = oneshot::channel();
    PENDING.with(|p| p.borrow_mut().insert(id, tx));

    let envelope = NativeEnvelope {
        id,
        request: request_b64,
    };
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    let frame_js =
        serde::Serialize::serialize(&envelope, &serializer).map_err(|e| e.to_string())?;

    tracing::trace!(id, "→ native host");
    ensure_port().post_message(&frame_js);

    let result = rx.await.map_err(|_| "native port closed".to_string())?;
    tracing::trace!(id, ok = result.is_ok(), "← native host");
    result
}

/// Encode a typed [`Request`], send it to the native host, and decode the
/// base64 reply into a [`Response`]. Used by the autofill handlers that
/// build their request structurally rather than relaying from the popup.
async fn call_native_request(request: Request) -> Result<Response, JsValue> {
    let b64 = call_native_raw(&request.encode_base64())
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
