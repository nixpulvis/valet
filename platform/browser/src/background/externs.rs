//! Raw `#[wasm_bindgen]` extern bindings into the WebExtensions `browser.*`
//! APIs the background script uses. Wrapped in safe helpers in [`super::port`].

use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    /// A `browser.runtime.Port` for native messaging.
    pub type Port;

    /// Send a JSON message over the port (`port.postMessage(msg)`).
    #[wasm_bindgen(method, js_name = postMessage)]
    pub fn post_message(this: &Port, msg: &JsValue);

    /// Access the `port.onMessage` event target.
    #[wasm_bindgen(method, getter, js_name = onMessage)]
    pub fn on_message(this: &Port) -> EventTarget;

    /// Access the `port.onDisconnect` event target.
    #[wasm_bindgen(method, getter, js_name = onDisconnect)]
    pub fn on_disconnect(this: &Port) -> EventTarget;

    /// The port's last error, if any.
    #[wasm_bindgen(method, getter)]
    pub fn error(this: &Port) -> JsValue;

    /// A browser event target that supports `addListener`.
    pub type EventTarget;

    /// Register a callback on this event target.
    #[wasm_bindgen(method, js_name = addListener)]
    pub fn add_listener(this: &EventTarget, cb: &::js_sys::Function);

    /// Open a native messaging port (`browser.runtime.connectNative`).
    #[wasm_bindgen(js_namespace = ["browser", "runtime"], js_name = connectNative)]
    pub fn connect_native(name: &str) -> Port;

    /// Register a listener on `browser.runtime.onMessage`.
    #[wasm_bindgen(js_namespace = ["browser", "runtime", "onMessage"], js_name = addListener)]
    pub fn runtime_on_message_add_listener(cb: &::js_sys::Function);
}
