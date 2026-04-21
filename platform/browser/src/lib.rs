//! Browser extension for the [valet] password manager.
//!
//! Compiled to WebAssembly and loaded by the extension's `background.js` and
//! `popup.html`. The two WASM entry points are [`background::start_background`]
//! and [`popup::start_popup`].
//!
//! Communication with the valet database goes through a native-messaging host
//! (`com.nixpulvis.valet`), with the background script acting as a multiplexing
//! RPC bridge between the popup UI and the host process.

use valet::password::Password;
use wasm_bindgen::prelude::*;

pub mod background;
pub mod content;
pub mod logging;
pub mod popup;
pub mod rpc;

/// Check whether a password candidate meets valet's minimum requirements.
///
/// Used by the popup to validate input before sending a `create_record` RPC.
#[wasm_bindgen]
pub fn password_is_valid(candidate: &str) -> bool {
    match Password::try_from(candidate) {
        Ok(password) => password.is_valid(),
        Err(_) => false,
    }
}

/// Generate a random 20-character password.
#[wasm_bindgen]
pub fn generate_password() -> String {
    Password::generate().as_str().to_string()
}

/// WASM module initialization hook
///
/// This runs automatically when the module is instantiated and sets up the
/// panic hook and logging system.
#[wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();

    let subsystem = web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .and_then(|p| p.rsplit('/').next().map(str::to_owned))
        .and_then(|f| f.split('.').next().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    logging::init(&subsystem);
}
