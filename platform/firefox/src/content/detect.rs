//! Login form detection and DOM observation.
//!
//! Scans the page for `<input type="password">` elements and attaches
//! Valet overlay icons. A [`MutationObserver`] re-scans when new nodes
//! are inserted (for SPAs that build forms dynamically).

use wasm_bindgen::prelude::*;
use web_sys::{Element, HtmlInputElement, MutationObserver, MutationObserverInit, Node};

use super::overlay;

const DETECTED_ATTR: &str = "data-valet-detected";

/// Minimum pixel dimensions for a field to be considered visible (not a honeypot).
const MIN_VISIBLE_WIDTH: i32 = 20;
const MIN_VISIBLE_HEIGHT: i32 = 10;

/// Run an initial scan and install a `MutationObserver` for future DOM changes.
pub(crate) fn scan_and_observe() {
    tracing::debug!("initial scan");
    scan();

    let cb = Closure::wrap(Box::new(|mutations: js_sys::Array, _observer: JsValue| {
        let dominated = mutations.iter().any(|m| {
            let record: web_sys::MutationRecord = m.unchecked_into();
            record.added_nodes().length() > 0
        });
        if dominated {
            tracing::trace!("DOM mutation detected, re-scanning");
            scan();
        }
    }) as Box<dyn FnMut(js_sys::Array, JsValue)>);

    let document = super::document();
    let observer =
        MutationObserver::new(cb.as_ref().unchecked_ref()).expect("MutationObserver::new");
    let opts = MutationObserverInit::new();
    opts.set_child_list(true);
    opts.set_subtree(true);
    observer
        .observe_with_options(document.document_element().unwrap().as_ref(), &opts)
        .expect("observe");
    cb.forget();
}

/// Scan the page for undiscovered password fields and attach overlays.
fn scan() {
    let document = super::document();

    // Match explicit password inputs and text inputs that look like
    // password fields (some sites use type="text" with a password-like
    // placeholder or name).
    let pw_fields = document
        .query_selector_all("input[type=\"password\"], input[type=\"text\"]")
        .unwrap();
    for i in 0..pw_fields.length() {
        let node = pw_fields.item(i).unwrap();
        let el: HtmlInputElement = node.unchecked_into();

        // For text inputs, only proceed if the placeholder or name
        // suggests it's actually a password field.
        if el.type_() == "text" && !looks_like_password(&el) {
            continue;
        }

        // Skip already-processed fields.
        if el.get_attribute(DETECTED_ATTR).is_some() {
            continue;
        }

        // Skip invisible / tiny fields (honeypots).
        let html_el: &web_sys::HtmlElement = el.as_ref();
        if html_el.offset_width() < MIN_VISIBLE_WIDTH || html_el.offset_height() < MIN_VISIBLE_HEIGHT {
            tracing::trace!(id = %el.id(), "skipping tiny/hidden password field");
            continue;
        }

        el.set_attribute(DETECTED_ATTR, "true").unwrap();

        // Find the most likely username field.
        let username_field = find_username_field(&el);
        tracing::debug!(
            pw_id = %el.id(),
            username_id = ?username_field.as_ref().map(|f| f.id()),
            "detected password field, attaching overlay"
        );

        overlay::attach(&el, username_field.as_ref());
    }
}

/// Walk up to the containing `<form>` (or parent) and find the most likely
/// username input: the last visible text/email input that precedes the
/// password field in DOM order.
fn find_username_field(pw: &HtmlInputElement) -> Option<HtmlInputElement> {
    let container: Element = pw
        .closest("form")
        .ok()
        .flatten()
        .or_else(|| pw.parent_element())?;

    let candidates = container
        .query_selector_all("input[type=\"text\"], input[type=\"email\"], input:not([type])")
        .ok()?;

    let pw_node: &Node = pw.as_ref();
    let mut best: Option<HtmlInputElement> = None;

    for i in 0..candidates.length() {
        let node = candidates.item(i).unwrap();
        let input: HtmlInputElement = node.clone().unchecked_into();
        let html: &web_sys::HtmlElement = input.as_ref();
        if html.offset_width() < MIN_VISIBLE_WIDTH || html.offset_height() < MIN_VISIBLE_HEIGHT {
            continue;
        }
        // Stop once we pass the password field in document order.
        let pos = pw_node.compare_document_position(node.as_ref());
        if pos & Node::DOCUMENT_POSITION_FOLLOWING != 0 {
            // `node` comes after `pw` — stop.
            break;
        }
        best = Some(input);
    }

    // Fallback: pick the first visible candidate if nothing precedes the pw field.
    if best.is_none() {
        for i in 0..candidates.length() {
            let node = candidates.item(i).unwrap();
            let input: HtmlInputElement = node.unchecked_into();
            let html: &web_sys::HtmlElement = input.as_ref();
            if html.offset_width() >= MIN_VISIBLE_WIDTH && html.offset_height() >= MIN_VISIBLE_HEIGHT {
                return Some(input);
            }
        }
    }

    best
}

/// Check whether a `type="text"` input is actually a disguised password field
/// based on its placeholder, name, or id.
fn looks_like_password(el: &HtmlInputElement) -> bool {
    for attr in ["placeholder", "name", "id", "aria-label"] {
        if let Some(val) = el.get_attribute(attr) {
            let val = val.to_lowercase();
            if val.contains("password") || val.contains("passwd") || val.contains("passwort") {
                return true;
            }
        }
    }
    false
}

