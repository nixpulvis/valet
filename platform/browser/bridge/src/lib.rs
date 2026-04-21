//! Native-messaging envelope types shared between the browser WASM
//! side and the native-host shim.
//!
//! The browser sends a [`NativeRequest`] as the JSON body of every
//! native-messaging frame; the shim replies with a [`NativeReply`].
//! The bitcode-encoded `valetd::Request` / `valetd::Response` bytes
//! travel as base64 strings inside the envelope so the native-
//! messaging layer can stay JSON.

use serde::{Deserialize, Serialize};

/// Request envelope posted from the browser to the native-host shim.
/// `request` is a base64-encoded bitcode `valetd::Request`; the shim
/// forwards the bytes without decoding them (socket backend) or decodes
/// them itself (embedded backend). `id` correlates the reply back to
/// the pending caller on the browser side.
#[derive(Serialize, Deserialize)]
pub struct NativeRequest {
    pub id: u32,
    pub request: String,
}

/// Reply envelope returned by the native-host shim. `backend` is the
/// transport that actually served the call (`"socket"` or
/// `"embedded"`). `payload` carries the base64-encoded bitcode
/// `valetd::Response` on success (with the request id echoed back) or
/// an error message on failure.
#[derive(Serialize, Deserialize)]
pub struct NativeReply {
    pub backend: String,
    pub payload: Result<NativePayload, String>,
}

/// Success body inside [`NativeReply::payload`].
#[derive(Serialize, Deserialize)]
pub struct NativePayload {
    pub id: u32,
    pub data: String,
}
