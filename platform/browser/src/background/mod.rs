//! Background script: opens the native messaging port to
//! `valet-native-host` (`com.nixpulvis.valet`) and multiplexes
//! RPC calls from the popup over it.

use wasm_bindgen::prelude::*;

pub(crate) mod externs;
pub(crate) mod port;

/// Initialize the background script: sets up logging and installs the
/// message listener that bridges popup RPC calls to the native host.
#[wasm_bindgen]
pub fn start_background() {
    tracing::info!("background script starting");
    port::install_message_listener();
}
