//! Fill detected form fields with credentials.
//!
//! Uses the native input setter and dispatches `input` + `change` events
//! so that JavaScript frameworks (React, Vue, etc.) pick up the change.

use wasm_bindgen::prelude::*;
use web_sys::HtmlInputElement;

/// Set the value of an input field, dispatching synthetic events.
pub(crate) fn set_field_value(field: &HtmlInputElement, value: &str) {
    let field_id = field.id();
    // Use the native HTMLInputElement.value setter to bypass
    // React/Vue controlled-component guards.
    if let Some(setter) = native_value_setter(field) {
        tracing::trace!(id = %field_id, "using native setter");
        setter.call1(field, &JsValue::from_str(value)).ok();
    } else {
        tracing::trace!(id = %field_id, "using fallback set_value");
        field.set_value(value);
    }

    let _ = field.dispatch_event(
        &web_sys::Event::new_with_event_init_dict("input", &event_init(true)).unwrap(),
    );
    let _ = field.dispatch_event(
        &web_sys::Event::new_with_event_init_dict("change", &event_init(true)).unwrap(),
    );
    tracing::trace!(id = %field_id, "dispatched input + change events");
}

/// Look up the native `value` setter on the HTMLInputElement prototype
/// so we can bypass framework property descriptors.
fn native_value_setter(el: &HtmlInputElement) -> Option<js_sys::Function> {
    let proto = js_sys::Object::get_prototype_of(el.as_ref());
    let desc = js_sys::Object::get_own_property_descriptor(&proto, &JsValue::from_str("value"));
    if desc.is_undefined() {
        return None;
    }
    js_sys::Reflect::get(&desc, &JsValue::from_str("set"))
        .ok()
        .and_then(|v| v.dyn_into::<js_sys::Function>().ok())
}

fn event_init(bubbles: bool) -> web_sys::EventInit {
    let init = web_sys::EventInit::new();
    init.set_bubbles(bubbles);
    init
}
