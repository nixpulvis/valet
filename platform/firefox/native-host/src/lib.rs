//! This crate has two halves:
//!
//! - **Library** (this module) — the shared wire types used by both the
//!   WASM addon and the native host. Wasm-friendly, no heavy dependencies.
//! - **Binary** (`valet-native-host`, in `src/main.rs`) — the native
//!   messaging host daemon that Firefox launches via
//!   `browser.runtime.connectNative("com.nixpulvis.valet")`. It speaks the
//!   WebExtensions native messaging wire format on stdin/stdout (4-byte
//!   little-endian length prefix + UTF-8 JSON). Gated behind the `bin`
//!   feature (on by default).
//!
//! The native messaging *frame* is JSON (Firefox enforces this), but the
//! successful `result` field is a base64-encoded bitcode blob of
//! [`RpcResult`]. The addon decodes that blob in WASM so we share one
//! schema across both halves.

use bitcode::{Decode, Encode};
use valet::record::Record;

/// The successful payload of any RPC call.
///
/// Each variant maps 1-to-1 with a native-host RPC method. The payload
/// uses valet's own types directly.
#[derive(Encode, Decode, Debug, PartialEq, Eq)]
pub enum RpcResult {
    /// Generic success with no extra data (e.g. `lock`, `lock_all`).
    Ok,
    /// List of all registered usernames (`list_users`).
    Users(Vec<String>),
    /// Currently unlocked usernames (`status`).
    Unlocked(Vec<String>),
    /// A record-bearing response.
    Record(RecordResult),
}

/// Record-specific RPC results.
#[derive(Encode, Decode, Debug, PartialEq, Eq)]
pub enum RecordResult {
    /// A single decrypted record (`get_record`).
    Get(Record),
    /// A newly created record (`create_record`).
    Created(Record),
    /// A record created with a generated password (`generate_record`).
    Generated(Record),
    // TODO: List should return record summaries (label + uuid) without
    // passwords, so we don't decrypt every record for a domain query.
    /// Matching records for a domain query (`find_records`).
    List(Vec<Record>),
}

/// Bitcode-encode an [`RpcResult`] and base64 it for the JSON envelope.
pub fn encode_result(result: &RpcResult) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let bytes = bitcode::encode(result);
    STANDARD.encode(bytes)
}

/// Decode a base64-encoded bitcode blob back into an [`RpcResult`].
pub fn decode_result(b64: &str) -> Result<RpcResult, DecodeError> {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let bytes = STANDARD.decode(b64).map_err(DecodeError::Base64)?;
    bitcode::decode(&bytes).map_err(DecodeError::Bitcode)
}

/// Errors that can occur when decoding a base64-bitcode [`RpcResult`].
#[derive(Debug)]
pub enum DecodeError {
    /// The base64 envelope was malformed.
    Base64(base64::DecodeError),
    /// The bitcode payload could not be decoded into [`RpcResult`].
    Bitcode(bitcode::Error),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Base64(e) => write!(f, "base64: {e}"),
            DecodeError::Bitcode(e) => write!(f, "bitcode: {e}"),
        }
    }
}
