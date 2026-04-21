//! Content script injected into web pages to detect login forms
//! and fill credentials from the Valet vault.
//!
//! Communicates with the background script via `browser.runtime.sendMessage`.
//! The background brokers RPC calls to the native host and returns
//! structured JSON responses.

use wasm_bindgen::prelude::*;
use web_sys::Document;

mod detect;
mod fill;
mod overlay;
mod rpc;

/// Entry point called from `content.js` after WASM initialisation.
#[wasm_bindgen]
pub fn start_content() {
    tracing::info!("content script starting");
    detect::scan_and_observe();
}

pub(crate) fn document() -> Document {
    web_sys::window().unwrap().document().unwrap()
}
