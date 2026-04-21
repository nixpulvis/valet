//! Typed RPC wrappers over [`browser::send_message`](super::browser::send_message).
//!
//! Each public function here sends `{ kind: "rpc", method, params }` to the
//! background script, which forwards the call to the native host. The
//! background script passes the raw base64-encoded bitcode result string
//! back, and we decode it here into a [`valetd::Response`] to extract
//! valet types directly.

use serde::Serialize;
use valet::{Record, record::Label, uuid::Uuid};
use valetd::{Response, request::Frame};
use wasm_bindgen::JsValue;

use super::browser;

/// Wrapper sent to the background script as `{ kind: "rpc", method, params }`.
#[derive(Serialize)]
pub(crate) struct BrowserEnvelope<'a, P: Serialize> {
    kind: &'static str,
    method: &'a str,
    params: P,
}

/// Send an RPC call and decode the base64 response into a [`Response`].
async fn call(method: &str, params: impl Serialize) -> Result<Response, String> {
    tracing::trace!(method, "rpc →");
    let envelope = BrowserEnvelope {
        kind: "rpc",
        method,
        params,
    };
    let js_result = browser::send_message(&envelope).await.map_err(js_err)?;
    let b64 = js_result
        .as_string()
        .ok_or_else(|| "expected base64 string from background".to_string())?;
    let response = Response::decode_base64(&b64).map_err(|e| e.to_string())?;
    tracing::trace!(method, "rpc ← ok");
    Ok(response)
}

fn js_err(v: JsValue) -> String {
    if let Some(s) = v.as_string() {
        return s;
    }
    if let Ok(msg) = js_sys::Reflect::get(&v, &JsValue::from_str("message")) {
        if let Some(s) = msg.as_string() {
            return s;
        }
    }
    format!("{v:?}")
}

#[derive(Serialize)]
struct Empty {}

/// List all registered usernames.
pub async fn list_users() -> Result<Vec<String>, String> {
    match call("list_users", Empty {}).await? {
        Response::Users(users) => Ok(users),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Return the list of currently unlocked usernames.
pub async fn status() -> Result<Vec<String>, String> {
    match call("status", Empty {}).await? {
        Response::Users(users) => Ok(users),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[derive(Serialize)]
struct UnlockParams<'a> {
    username: &'a str,
    password: &'a str,
}

/// Unlock a user's vault with the given password.
pub async fn unlock(username: &str, password: &str) -> Result<(), String> {
    match call("unlock", UnlockParams { username, password }).await? {
        Response::Ok => Ok(()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Lock all currently unlocked users.
pub async fn lock_all() -> Result<(), String> {
    match call("lock_all", Empty {}).await? {
        Response::Ok => Ok(()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[derive(Serialize)]
struct FindParams<'a> {
    username: &'a str,
    lot: &'a str,
    domain: &'a str,
}

/// Find records matching a domain in the given user's lot. Returns only
/// the label-and-uuid pairs; passwords are fetched on demand via
/// [`get_record`].
pub async fn find_records(
    username: &str,
    lot: &str,
    domain: &str,
) -> Result<Vec<(Uuid<Record>, Label)>, String> {
    match call(
        "find_records",
        FindParams {
            username,
            lot,
            domain,
        },
    )
    .await?
    {
        Response::Index(entries) => Ok(entries),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[derive(Serialize)]
struct GetRecordParams<'a> {
    username: &'a str,
    lot: &'a str,
    record_uuid: &'a str,
}

/// Fetch a full decrypted record by UUID.
pub async fn get_record(username: &str, lot: &str, record_uuid: &str) -> Result<Record, String> {
    match call(
        "get_record",
        GetRecordParams {
            username,
            lot,
            record_uuid,
        },
    )
    .await?
    {
        Response::Record(record) => Ok(record),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[derive(Serialize)]
struct CreateParams<'a> {
    username: &'a str,
    lot: &'a str,
    label: &'a str,
    password: &'a str,
}

/// Create a new credential record in the given lot.
pub async fn create_record(
    username: &str,
    lot: &str,
    label: &str,
    password: &str,
) -> Result<Record, String> {
    match call(
        "create_record",
        CreateParams {
            username,
            lot,
            label,
            password,
        },
    )
    .await?
    {
        Response::Record(record) => Ok(record),
        other => Err(format!("unexpected response: {other:?}")),
    }
}
