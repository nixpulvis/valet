//! Thin async wrappers over the Firefox WebExtension APIs the popup uses:
//! message passing, tab queries, and clipboard access.

use serde::Serialize;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["browser", "runtime"], js_name = sendMessage, catch)]
    fn runtime_send_message(msg: JsValue) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_namespace = ["browser", "tabs"], js_name = query, catch)]
    fn tabs_query(filter: JsValue) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_namespace = ["browser", "permissions"], js_name = contains, catch)]
    fn permissions_contains(perms: JsValue) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_namespace = ["browser", "permissions"], js_name = request, catch)]
    fn permissions_request(perms: JsValue) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_namespace = ["browser", "storage", "local"], js_name = get, catch)]
    fn storage_local_get(keys: JsValue) -> Result<js_sys::Promise, JsValue>;

    #[wasm_bindgen(js_namespace = ["browser", "storage", "local"], js_name = set, catch)]
    fn storage_local_set(items: JsValue) -> Result<js_sys::Promise, JsValue>;
}

/// Send a message to the background script via `browser.runtime.sendMessage`.
pub async fn send_message<T: Serialize>(msg: &T) -> Result<JsValue, JsValue> {
    let value = serde_wasm_bindgen::to_value(msg).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let promise = runtime_send_message(value)?;
    JsFuture::from(promise).await
}

#[derive(Serialize)]
struct TabsQuery {
    active: bool,
    #[serde(rename = "currentWindow")]
    current_window: bool,
}

/// Return the hostname of the active tab in the current window, if any.
pub async fn current_tab_domain() -> Option<String> {
    let filter = serde_wasm_bindgen::to_value(&TabsQuery {
        active: true,
        current_window: true,
    })
    .ok()?;
    let promise = tabs_query(filter).ok()?;
    let tabs = JsFuture::from(promise).await.ok()?;
    let tab = js_sys::Array::from(&tabs).get(0);
    if tab.is_undefined() {
        return None;
    }
    let url = js_sys::Reflect::get(&tab, &JsValue::from_str("url")).ok()?;
    let url = url.as_string()?;
    web_sys::Url::new(&url)
        .ok()
        .map(|u| u.hostname().to_lowercase())
}

/// Check whether the extension has host permissions for all sites.
pub async fn has_host_permissions() -> bool {
    let perms = origins_object();
    let Ok(promise) = permissions_contains(perms) else {
        return false;
    };
    JsFuture::from(promise)
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Request host permissions for all sites. Must be called from a user gesture.
pub async fn request_host_permissions() -> bool {
    let perms = origins_object();
    let Ok(promise) = permissions_request(perms) else {
        return false;
    };
    JsFuture::from(promise)
        .await
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn origins_object() -> JsValue {
    let origins = js_sys::Array::new();
    origins.push(&JsValue::from_str("*://*/*"));
    let obj = js_sys::Object::new();
    js_sys::Reflect::set(&obj, &JsValue::from_str("origins"), &origins).unwrap();
    obj.into()
}

const DISMISSED_KEY: &str = "permissions_banner_dismissed";

/// Check whether the user previously dismissed the permissions banner.
pub async fn permissions_banner_dismissed() -> bool {
    let Ok(promise) = storage_local_get(JsValue::from_str(DISMISSED_KEY)) else {
        return false;
    };
    let Ok(result) = JsFuture::from(promise).await else {
        return false;
    };
    js_sys::Reflect::get(&result, &JsValue::from_str(DISMISSED_KEY))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Persist that the user dismissed the permissions banner.
pub async fn dismiss_permissions_banner() {
    let obj = js_sys::Object::new();
    js_sys::Reflect::set(&obj, &JsValue::from_str(DISMISSED_KEY), &JsValue::TRUE).unwrap();
    if let Ok(promise) = storage_local_set(obj.into()) {
        let _ = JsFuture::from(promise).await;
    }
}

/// Write `text` to the system clipboard via `navigator.clipboard.writeText`.
pub async fn copy_to_clipboard(text: &str) -> Result<(), JsValue> {
    let clipboard = web_sys::window()
        .ok_or_else(|| JsValue::from_str("no window"))?
        .navigator()
        .clipboard();
    JsFuture::from(clipboard.write_text(text)).await.map(|_| ())
}
