//! Message passing to the background script for autofill operations.

use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["browser", "runtime"], js_name = sendMessage, catch)]
    fn runtime_send_message(msg: JsValue) -> Result<js_sys::Promise, JsValue>;
}

async fn send<T: Serialize>(msg: &T) -> Result<JsValue, JsValue> {
    let value = serde_wasm_bindgen::to_value(msg).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let promise = runtime_send_message(value)?;
    JsFuture::from(promise).await
}

/// A credential summary returned by the background script.
#[derive(Deserialize, Debug, Clone)]
pub(crate) struct Credential {
    pub label: String,
    pub record_uuid: String,
}

/// Check whether a user is currently unlocked.
pub(crate) async fn autofill_status() -> Result<Option<String>, String> {
    tracing::trace!("rpc → autofill_status");
    #[derive(Serialize)]
    struct Msg {
        kind: &'static str,
    }
    let resp = send(&Msg {
        kind: "autofill_status",
    })
    .await
    .map_err(js_err)?;
    let obj: StatusResponse = serde_wasm_bindgen::from_value(resp).map_err(|e| e.to_string())?;
    tracing::trace!(
        unlocked = obj.username.is_some(),
        "({}) rpc ← autofill_status",
        obj.backend,
    );
    Ok(obj.username)
}

#[derive(Deserialize)]
struct StatusResponse {
    username: Option<String>,
    backend: String,
}

/// Request matching credentials for the given domain.
pub(crate) async fn autofill_request(domain: &str) -> Result<Vec<Credential>, String> {
    tracing::trace!(domain = %domain, "rpc → autofill_request");
    #[derive(Serialize)]
    struct Msg<'a> {
        kind: &'static str,
        domain: &'a str,
    }
    let resp = send(&Msg {
        kind: "autofill_request",
        domain,
    })
    .await
    .map_err(js_err)?;
    let obj: RequestResponse = serde_wasm_bindgen::from_value(resp).map_err(|e| e.to_string())?;
    if let Some(err) = obj.error {
        tracing::trace!(
            domain = %domain,
            error = %err,
            "({}) rpc ← autofill_request error",
            obj.backend,
        );
        return Err(err);
    }
    tracing::trace!(
        domain = %domain,
        count = obj.credentials.len(),
        "({}) rpc ← autofill_request",
        obj.backend,
    );
    Ok(obj.credentials)
}

#[derive(Deserialize)]
struct RequestResponse {
    #[serde(default)]
    credentials: Vec<Credential>,
    error: Option<String>,
    backend: String,
}

/// Fetch the decrypted credential (username parsed from label + password)
/// for a specific record.
pub(crate) async fn autofill_fill(record_uuid: &str) -> Result<FillData, String> {
    tracing::trace!(record_uuid = %record_uuid, "rpc → autofill_fill");
    #[derive(Serialize)]
    struct Msg<'a> {
        kind: &'static str,
        record_uuid: &'a str,
    }
    let resp = send(&Msg {
        kind: "autofill_fill",
        record_uuid,
    })
    .await
    .map_err(js_err)?;
    let obj: FillData = serde_wasm_bindgen::from_value(resp).map_err(|e| e.to_string())?;
    if let Some(ref err) = obj.error {
        tracing::trace!(
            record_uuid = %record_uuid,
            error = %err,
            "({}) rpc ← autofill_fill error",
            obj.backend,
        );
        return Err(err.clone());
    }
    tracing::trace!(
        record_uuid = %record_uuid,
        "({}) rpc ← autofill_fill ok",
        obj.backend,
    );
    Ok(obj)
}

/// A credential returned by the background for autofill fill/generate.
#[derive(Deserialize, Debug)]
pub(crate) struct FillData {
    pub username: Option<String>,
    pub password: String,
    pub error: Option<String>,
    pub backend: String,
}

/// Generate a password, save it as a record with the given label, and
/// return the credential for filling.
pub(crate) async fn autofill_generate(label: &str) -> Result<FillData, String> {
    tracing::trace!(label = %label, "rpc → autofill_generate");
    #[derive(Serialize)]
    struct Msg<'a> {
        kind: &'static str,
        label: &'a str,
    }
    let resp = send(&Msg {
        kind: "autofill_generate",
        label,
    })
    .await
    .map_err(js_err)?;
    let obj: FillData = serde_wasm_bindgen::from_value(resp).map_err(|e| e.to_string())?;
    if let Some(ref err) = obj.error {
        tracing::trace!(
            label = %label,
            error = %err,
            "({}) rpc ← autofill_generate error",
            obj.backend,
        );
        return Err(err.clone());
    }
    tracing::trace!(
        label = %label,
        "({}) rpc ← autofill_generate ok",
        obj.backend,
    );
    Ok(obj)
}

fn js_err(v: JsValue) -> String {
    v.as_string().unwrap_or_else(|| format!("{v:?}"))
}
