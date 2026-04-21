//! Popup UI rendered with [Yew](yew) inside the extension's popup window.
//!
//! The popup communicates with the background script via
//! `browser.runtime.sendMessage`, using typed RPC wrappers internally.

use wasm_bindgen::prelude::*;

pub(crate) mod app;
pub(crate) mod browser;

/// Mount the popup Yew application onto the `#root` DOM element.
#[wasm_bindgen]
pub fn start_popup() {
    tracing::info!("popup mounting");
    let root = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("root"))
        .expect("popup root element");
    yew::Renderer::<app::App>::with_root(root).render();
}
