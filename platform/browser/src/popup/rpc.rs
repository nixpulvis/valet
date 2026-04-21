//! Typed RPC wrappers over [`browser::send_message`](super::browser::send_message).
//!
//! Each public function here builds a [`valetd::Request`], encodes it as
//! base64 via [`Frame::encode_base64`], and sends
//! `{ kind: "rpc", request: <base64> }` to the background script. The
//! background forwards the encoded request to the native host without
//! decoding it. The base64 [`Response`] that comes back is decoded here
//! and matched for the expected variant.
//!
//! Adding an RPC is one new `valetd::Request` variant plus one wrapper
//! here — the background and native host don't need to learn about it.

use serde::Serialize;
use std::collections::HashMap;
use std::str::FromStr;
use valet::{Record, password::Password, record::Label, uuid::Uuid};
use valetd::{Request, Response, request::Frame};
use wasm_bindgen::JsValue;

use super::browser;

/// Wrapper sent to the background script. `kind` is always `"rpc"` here;
/// other kinds (autofill) are handled by dedicated message handlers in the
/// background script and don't pass through this module.
#[derive(Serialize)]
pub(crate) struct BrowserEnvelope<'a> {
    kind: &'static str,
    request: &'a str,
}

/// Send a [`Request`] to the background and decode the base64 reply into
/// a [`Response`].
async fn call(request: Request) -> Result<Response, String> {
    let request_b64 = request.encode_base64();
    let envelope = BrowserEnvelope {
        kind: "rpc",
        request: &request_b64,
    };
    let js_result = browser::send_message(&envelope).await.map_err(js_err)?;
    let b64 = js_result
        .as_string()
        .ok_or_else(|| "expected base64 string from background".to_string())?;
    Response::decode_base64(&b64).map_err(|e| e.to_string())
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

/// List all registered usernames.
pub async fn list_users() -> Result<Vec<String>, String> {
    match call(Request::ListUsers).await? {
        Response::Users(users) => Ok(users),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Return the list of currently unlocked usernames.
pub async fn status() -> Result<Vec<String>, String> {
    match call(Request::Status).await? {
        Response::Users(users) => Ok(users),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Unlock a user's vault with the given password.
pub async fn unlock(username: &str, password: &str) -> Result<(), String> {
    let password: Password = password.try_into().map_err(|_| "password too long".to_string())?;
    match call(Request::Unlock {
        username: username.to_owned(),
        password,
    })
    .await?
    {
        Response::Ok => Ok(()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Lock all currently unlocked users.
pub async fn lock_all() -> Result<(), String> {
    match call(Request::LockAll).await? {
        Response::Ok => Ok(()),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Find records matching a domain in the given user's lot. Returns only
/// the label-and-uuid pairs; passwords are fetched on demand via
/// [`get_record`].
pub async fn find_records(
    username: &str,
    lot: &str,
    domain: &str,
) -> Result<Vec<(Uuid<Record>, Label)>, String> {
    match call(Request::FindRecords {
        username: username.to_owned(),
        lot: lot.to_owned(),
        query: domain.to_owned(),
    })
    .await?
    {
        Response::Index(entries) => Ok(entries),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Fetch a full decrypted record by UUID.
pub async fn get_record(username: &str, lot: &str, record_uuid: &str) -> Result<Record, String> {
    let uuid: Uuid<Record> = Uuid::parse(record_uuid).map_err(|e| format!("{e:?}"))?;
    match call(Request::GetRecord {
        username: username.to_owned(),
        lot: lot.to_owned(),
        uuid,
    })
    .await?
    {
        Response::Record(record) => Ok(record),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

/// Create a new credential record in the given lot.
pub async fn create_record(
    username: &str,
    lot: &str,
    label: &str,
    password: &str,
) -> Result<Record, String> {
    let label = Label::from_str(label).map_err(|e| format!("{e:?}"))?;
    let password: Password = password.try_into().map_err(|_| "password too long".to_string())?;
    match call(Request::CreateRecord {
        username: username.to_owned(),
        lot: lot.to_owned(),
        label,
        password,
        extra: HashMap::<String, String>::new(),
    })
    .await?
    {
        Response::Record(record) => Ok(record),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

