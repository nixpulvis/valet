//! Overlay icon and credential picker attached to detected password fields.
//!
//! Each password field gets a small "V" button positioned at its right edge
//! inside a shadow DOM host (so page CSS cannot interfere). Clicking the
//! button shows a menu with saved credentials and a "Generate password" option.

use std::cell::RefCell;
use std::rc::Rc;

use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{Document, Element, HtmlElement, HtmlInputElement, ShadowRootInit, ShadowRootMode};

use super::{fill, rpc};

/// Z-index for overlay elements — maximum i32 to sit above all page content.
const OVERLAY_Z_INDEX: &str = "2147483647";

/// Size of the Valet icon button in pixels.
const ICON_SIZE: f64 = 24.0;

/// Inset from the right edge of the password field to the icon center.
const ICON_RIGHT_INSET: f64 = 28.0;

/// Minimum width of the credential dropdown menu in pixels.
const DROPDOWN_MIN_WIDTH: f64 = 200.0;

/// How long (ms) to show the toast before fading.
const TOAST_VISIBLE_MS: u32 = 3000;

/// Fade-out transition duration (ms) after the toast starts fading.
const TOAST_FADE_MS: u32 = 300;

/// How long (ms) to highlight the username field when it's empty.
const HIGHLIGHT_MS: u32 = 3_000;

/// Attach a Valet overlay icon to a password field.
pub(crate) fn attach(pw: &HtmlInputElement, username: Option<&HtmlInputElement>) {
    let document = super::document();

    // Shadow DOM host keeps our UI isolated from the page.
    // Uses position:absolute so it's placed in page (document) coordinates
    // and naturally scrolls with the content — immune to macOS elastic
    // overscroll drift that affects position:fixed elements.
    let host = document.create_element("div").unwrap();
    host.set_attribute(
        "style",
        &format!("position:absolute;z-index:{OVERLAY_Z_INDEX};pointer-events:none;"),
    )
    .unwrap();

    let shadow_init = ShadowRootInit::new(ShadowRootMode::Closed);
    let shadow = host.attach_shadow(&shadow_init).unwrap();

    let btn = document.create_element("button").unwrap();
    btn.set_inner_html(ICON_SVG);
    btn.set_attribute("title", "Valet").unwrap();
    btn.set_attribute("style", BTN_STYLE).unwrap();

    // Hover effect.
    let btn_el: HtmlElement = btn.clone().unchecked_into();
    {
        let btn_el = btn_el.clone();
        let enter = Closure::wrap(Box::new(move || {
            btn_el.style().set_property("opacity", "1").unwrap();
        }) as Box<dyn FnMut()>);
        btn.add_event_listener_with_callback("mouseenter", enter.as_ref().unchecked_ref())
            .unwrap();
        enter.forget();
    }
    {
        let btn_el = btn_el.clone();
        let leave = Closure::wrap(Box::new(move || {
            btn_el.style().set_property("opacity", "0.7").unwrap();
        }) as Box<dyn FnMut()>);
        btn.add_event_listener_with_callback("mouseleave", leave.as_ref().unchecked_ref())
            .unwrap();
        leave.forget();
    }

    // Click handler — show the action menu.
    let pw_clone = pw.clone();
    let username_clone = username.cloned();
    let click = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
        e.prevent_default();
        e.stop_propagation();
        tracing::debug!("valet icon clicked");
        let pw = pw_clone.clone();
        let username = username_clone.clone();
        spawn_local(async move {
            show_menu(&pw, username.as_ref()).await;
        });
    }) as Box<dyn FnMut(web_sys::MouseEvent)>);
    btn.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())
        .unwrap();
    click.forget();

    shadow.append_child(&btn).unwrap();
    position_host(&host, pw);

    document
        .document_element()
        .unwrap()
        .append_child(&host)
        .unwrap();

    // Continuously track the input's position via requestAnimationFrame.
    // This handles all repositioning cases: scrollable parent containers,
    // CSS animations, JS-driven layout changes, window resize, etc.
    start_position_loop(host, pw.clone());
}

fn position_host(host: &Element, pw: &HtmlInputElement) {
    let rect = pw.get_bounding_client_rect();

    // Hide when the input is invisible (zero-size rect).
    let visible = rect.width() > 0.0 && rect.height() > 0.0;
    let html: &HtmlElement = host.unchecked_ref();
    html.style()
        .set_property("display", if visible { "" } else { "none" })
        .unwrap();
    if !visible {
        return;
    }

    // Convert viewport-relative rect to page (document) coordinates by
    // adding the current scroll offset. This pairs with position:absolute
    // on the host, so the icon scrolls naturally with the page and is
    // immune to macOS elastic overscroll drift.
    let window = web_sys::window().unwrap();
    let scroll_x = window.scroll_x().unwrap_or(0.0);
    let scroll_y = window.scroll_y().unwrap_or(0.0);

    let top = rect.top() + scroll_y + (rect.height() - ICON_SIZE) / 2.0;
    let left = rect.right() + scroll_x - ICON_RIGHT_INSET;

    html.style()
        .set_property("top", &format!("{top}px"))
        .unwrap();
    html.style()
        .set_property("left", &format!("{left}px"))
        .unwrap();
}

/// Run a `requestAnimationFrame` loop that keeps the overlay host positioned
/// over the password field. Stops automatically when the input is removed
/// from the DOM.
fn start_position_loop(host: Element, pw: HtmlInputElement) {
    // Shared slot for the rAF closure so it can re-register itself.
    let cb: Rc<RefCell<Option<Closure<dyn FnMut()>>>> = Rc::new(RefCell::new(None));
    let cb_clone = cb.clone();

    // Anchor DOM references on the JS heap so Safari's GC won't collect
    // them from the WASM externref table.
    let anchor = js_sys::Array::of2(host.as_ref(), pw.as_ref());

    let closure = Closure::wrap(Box::new(move || {
        // Keep anchor alive — prevents Safari GC from collecting the refs.
        let _ = &anchor;

        // Stop if the input was removed from the document.
        if !pw.is_connected() {
            host.remove();
            // Drop the closure to break the prevent leak.
            cb_clone.borrow_mut().take();
            return;
        }

        position_host(&host, &pw);

        // Schedule the next frame.
        if let Some(ref c) = *cb_clone.borrow() {
            web_sys::window()
                .unwrap()
                .request_animation_frame(c.as_ref().unchecked_ref())
                .unwrap();
        }
    }) as Box<dyn FnMut()>);

    // Kick off the first frame.
    web_sys::window()
        .unwrap()
        .request_animation_frame(closure.as_ref().unchecked_ref())
        .unwrap();

    *cb.borrow_mut() = Some(closure);
}

/// Show the Valet action menu below the password field.
///
/// Always includes "Generate password". If the vault is unlocked and
/// there are matching credentials for the current domain, those are
/// listed above it.
async fn show_menu(pw: &HtmlInputElement, username: Option<&HtmlInputElement>) {
    let domain = web_sys::window()
        .unwrap()
        .location()
        .hostname()
        .unwrap_or_default();
    tracing::debug!(domain = %domain, "showing valet menu");

    // Try to fetch credentials if unlocked — but don't block the menu
    // on this. We always show "Generate password" regardless.
    // Check if a user is unlocked.
    match rpc::autofill_status().await {
        Ok(Some(u)) => {
            tracing::debug!(username = %u, "vault unlocked");
        }
        Ok(None) => {
            tracing::debug!("vault is locked");
            show_toast("Valet is locked. Click the Valet toolbar icon to unlock.");
            return;
        }
        Err(e) => {
            tracing::warn!(error = %e, "autofill status check failed");
            return;
        }
    };

    let credentials = match rpc::autofill_request(&domain).await {
        Ok(c) => {
            tracing::debug!(domain = %domain, count = c.len(), "credentials found");
            c
        }
        Err(e) => {
            tracing::warn!(domain = %domain, error = %e, "autofill request failed");
            Vec::new()
        }
    };

    show_dropdown(pw, username, &credentials).await;
}

/// Render the dropdown menu.
async fn show_dropdown(
    pw: &HtmlInputElement,
    username: Option<&HtmlInputElement>,
    credentials: &[rpc::Credential],
) {
    let document = super::document();
    let rect = pw.get_bounding_client_rect();

    let window = web_sys::window().unwrap();
    let scroll_x = window.scroll_x().unwrap_or(0.0);
    let scroll_y = window.scroll_y().unwrap_or(0.0);

    let host = document.create_element("div").unwrap();
    host.set_attribute(
        "style",
        &format!(
            "position:absolute;z-index:{OVERLAY_Z_INDEX};pointer-events:auto;\
             top:{}px;left:{}px;",
            rect.bottom() + scroll_y + 2.0,
            rect.left() + scroll_x,
        ),
    )
    .unwrap();

    let shadow = host
        .attach_shadow(&ShadowRootInit::new(ShadowRootMode::Closed))
        .unwrap();

    let list = document.create_element("div").unwrap();
    let min_width = rect.width().max(DROPDOWN_MIN_WIDTH);
    list.set_attribute(
        "style",
        &format!(
            "all:initial;display:flex;flex-direction:column;\
             background:#fff;border:1px solid #d1d5db;border-radius:6px;\
             box-shadow:0 4px 12px rgba(0,0,0,0.15);\
             font-family:system-ui,sans-serif;font-size:13px;\
             max-height:200px;overflow-y:auto;min-width:{min_width}px;",
        ),
    )
    .unwrap();

    let host_ref = Rc::new(RefCell::new(Some(host.clone())));

    // Credential items.
    for cred in credentials {
        let item = make_menu_item(&document, &cred.label);

        let pw = pw.clone();
        let username = username.cloned();
        let uuid = cred.record_uuid.clone();
        let host_ref = host_ref.clone();
        let click = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            e.prevent_default();
            e.stop_propagation();
            if let Some(h) = host_ref.borrow_mut().take() {
                h.remove();
            }
            let pw = pw.clone();
            let username = username.clone();
            let uuid = uuid.clone();
            spawn_local(async move {
                do_fill(&pw, username.as_ref(), &uuid).await;
            });
        }) as Box<dyn FnMut(web_sys::MouseEvent)>);
        item.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())
            .unwrap();
        click.forget();

        list.append_child(&item).unwrap();
    }

    // Separator if there were credentials above.
    if !credentials.is_empty() {
        let sep = document.create_element("div").unwrap();
        sep.set_attribute(
            "style",
            "all:initial;border-top:1px solid #e5e7eb;margin:0;",
        )
        .unwrap();
        list.append_child(&sep).unwrap();
    }

    // "Generate password" item.
    {
        let item = make_menu_item(&document, "Generate password");
        let pw = pw.clone();
        let username = username.cloned();
        let host_ref = host_ref.clone();
        let click = Closure::wrap(Box::new(move |e: web_sys::MouseEvent| {
            e.prevent_default();
            e.stop_propagation();
            if let Some(h) = host_ref.borrow_mut().take() {
                h.remove();
            }
            let pw = pw.clone();
            let username = username.clone();
            spawn_local(async move {
                do_generate(&pw, username.as_ref()).await;
            });
        }) as Box<dyn FnMut(web_sys::MouseEvent)>);
        item.add_event_listener_with_callback("click", click.as_ref().unchecked_ref())
            .unwrap();
        click.forget();
        list.append_child(&item).unwrap();
    }

    shadow.append_child(&list).unwrap();
    document
        .document_element()
        .unwrap()
        .append_child(&host)
        .unwrap();

    // Close on outside click. Deferred via setTimeout so the current
    // click event doesn't immediately trigger removal.
    let host_ref = Rc::new(RefCell::new(Some(host)));
    let host_ref_close = host_ref.clone();
    let close = Closure::wrap(Box::new(move |_e: web_sys::Event| {
        if let Some(h) = host_ref_close.borrow_mut().take() {
            h.remove();
        }
    }) as Box<dyn FnMut(web_sys::Event)>);
    let close_fn: js_sys::Function = close.as_ref().unchecked_ref::<js_sys::Function>().clone();
    close.forget();
    let window = web_sys::window().unwrap();
    let register = Closure::once(Box::new(move || {
        let opts = web_sys::AddEventListenerOptions::new();
        opts.set_once(true);
        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .add_event_listener_with_callback_and_add_event_listener_options(
                "click", &close_fn, &opts,
            )
            .unwrap();
    }) as Box<dyn FnOnce()>);
    window
        .set_timeout_with_callback_and_timeout_and_arguments_0(register.as_ref().unchecked_ref(), 0)
        .unwrap();
    register.forget();
}

/// Create a styled menu item button.
fn make_menu_item(document: &Document, label: &str) -> Element {
    let item = document.create_element("button").unwrap();
    item.set_text_content(Some(label));
    item.set_attribute("style", MENU_ITEM_STYLE).unwrap();

    let item_html: HtmlElement = item.clone().unchecked_into();
    {
        let el = item_html.clone();
        let enter = Closure::wrap(Box::new(move || {
            el.style().set_property("background", "#f3f4f6").unwrap();
        }) as Box<dyn FnMut()>);
        item.add_event_listener_with_callback("mouseenter", enter.as_ref().unchecked_ref())
            .unwrap();
        enter.forget();
    }
    {
        let el = item_html;
        let leave = Closure::wrap(Box::new(move || {
            el.style()
                .set_property("background", "transparent")
                .unwrap();
        }) as Box<dyn FnMut()>);
        item.add_event_listener_with_callback("mouseleave", leave.as_ref().unchecked_ref())
            .unwrap();
        leave.forget();
    }

    item
}

/// Generate a password via the native host, save it as a record, and
/// fill the password field.
///
/// Requires the username/email field to be filled so we can build the
/// `id@domain` label for the record.
async fn do_generate(pw: &HtmlInputElement, username_field: Option<&HtmlInputElement>) {
    // Read the username value from the form.
    let id = username_field.map(|f| f.value().trim().to_string());
    if id.as_ref().is_none_or(|s| s.is_empty()) {
        tracing::debug!("generate: username field is empty");
        // Highlight the username field.
        if let Some(u_field) = username_field {
            let html: &HtmlElement = u_field.unchecked_ref();
            html.style()
                .set_property("outline", "2px solid #b00020")
                .unwrap();
            html.style().set_property("outline-offset", "-2px").unwrap();
            u_field.focus().ok();
            // Remove highlight after 3 seconds.
            let u_field = u_field.clone();
            spawn_local(async move {
                gloo_timers::future::TimeoutFuture::new(HIGHLIGHT_MS).await;
                let html: &HtmlElement = u_field.unchecked_ref();
                html.style().remove_property("outline").ok();
                html.style().remove_property("outline-offset").ok();
            });
        }
        show_toast("Fill in the username/email first.");
        return;
    }
    let id = id.unwrap();

    let domain = web_sys::window()
        .unwrap()
        .location()
        .hostname()
        .unwrap_or_default();
    let label = format!("{id}@{domain}");

    tracing::debug!(label = %label, "generating password");
    match rpc::autofill_generate(&label).await {
        Ok(data) => {
            tracing::info!(label = %label, "password generated and saved");
            fill::set_field_value(pw, &data.password);
        }
        Err(e) => {
            tracing::warn!(label = %label, error = %e, "generate failed");
            show_toast(&format!("Generate failed: {e}"));
        }
    }
}

/// Fill the form with a specific credential.
async fn do_fill(
    pw: &HtmlInputElement,
    username_field: Option<&HtmlInputElement>,
    record_uuid: &str,
) {
    tracing::debug!(record_uuid = %record_uuid, "filling credential");
    match rpc::autofill_fill(record_uuid).await {
        Ok(data) => {
            if let Some(u_field) = username_field {
                if let Some(ref id) = data.username {
                    tracing::debug!(id = %id, "filling username field");
                    fill::set_field_value(u_field, id);
                }
            }
            tracing::debug!("filling password field");
            fill::set_field_value(pw, &data.password);
            tracing::info!("autofill complete");
        }
        Err(e) => {
            tracing::warn!(record_uuid = %record_uuid, error = %e, "fill failed");
            show_toast(&format!("Fill failed: {e}"));
        }
    }
}

/// Show a temporary toast notification.
fn show_toast(text: &str) {
    let document = super::document();
    let host = document.create_element("div").unwrap();
    host.set_attribute(
        "style",
        &format!(
            "position:fixed;top:12px;right:12px;z-index:{OVERLAY_Z_INDEX};pointer-events:none;"
        ),
    )
    .unwrap();

    let shadow = host
        .attach_shadow(&ShadowRootInit::new(ShadowRootMode::Closed))
        .unwrap();

    let msg = document.create_element("div").unwrap();
    msg.set_text_content(Some(text));
    msg.set_attribute("style", TOAST_STYLE).unwrap();

    shadow.append_child(&msg).unwrap();
    document
        .document_element()
        .unwrap()
        .append_child(&host)
        .unwrap();

    let msg_html: HtmlElement = msg.unchecked_into();
    let host_clone = host.clone();
    let fade = Closure::once(Box::new(move || {
        msg_html.style().set_property("opacity", "0").unwrap();
        let remove = Closure::once(Box::new(move || {
            host_clone.remove();
        }) as Box<dyn FnOnce()>);
        web_sys::window()
            .unwrap()
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                remove.as_ref().unchecked_ref(),
                TOAST_FADE_MS as i32,
            )
            .unwrap();
        remove.forget();
    }) as Box<dyn FnOnce()>);
    web_sys::window()
        .unwrap()
        .set_timeout_with_callback_and_timeout_and_arguments_0(
            fade.as_ref().unchecked_ref(),
            TOAST_VISIBLE_MS as i32,
        )
        .unwrap();
    fade.forget();
}

const ICON_SVG: &str = "\
<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 20\" width=\"18\" height=\"18\">\
<rect x=\"1\" y=\"1\" width=\"18\" height=\"18\" rx=\"4\" fill=\"#4a5568\" opacity=\"0.9\"/>\
<text x=\"10\" y=\"14.5\" text-anchor=\"middle\" fill=\"white\" \
font-family=\"system-ui,sans-serif\" font-size=\"11\" font-weight=\"bold\">V</text>\
</svg>";

const BTN_STYLE: &str = "\
    all:initial;\
    cursor:pointer;\
    pointer-events:auto;\
    display:flex;\
    align-items:center;\
    justify-content:center;\
    width:24px;\
    height:24px;\
    border:none;\
    background:transparent;\
    padding:0;\
    opacity:0.7;\
    transition:opacity 0.15s;\
";

const MENU_ITEM_STYLE: &str = "\
    all:initial;\
    display:block;\
    width:100%;\
    padding:8px 12px;\
    text-align:left;\
    cursor:pointer;\
    font-family:system-ui,sans-serif;\
    font-size:13px;\
    border:none;\
    background:transparent;\
    color:#1f2937;\
";

const TOAST_STYLE: &str = "\
    all:initial;\
    display:inline-block;\
    padding:10px 16px;\
    background:#1f2937;\
    color:#fff;\
    font-family:system-ui,sans-serif;\
    font-size:13px;\
    border-radius:8px;\
    box-shadow:0 4px 12px rgba(0,0,0,0.2);\
    opacity:1;\
    transition:opacity 0.3s;\
";
