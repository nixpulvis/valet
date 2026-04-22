//! JS bindings the background script still needs directly. The
//! `browser.runtime.Port` bindings live inside
//! [`valet::protocol::native_msg`] along with `Client<NativeMessage>`;
//! only the popup/content-facing `onMessage` registration remains here.

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// Register a listener on `browser.runtime.onMessage`.
    #[wasm_bindgen(js_namespace = ["browser", "runtime", "onMessage"], js_name = addListener)]
    pub fn runtime_on_message_add_listener(cb: &::js_sys::Function);
}
