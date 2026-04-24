//! `browser.runtime.onMessage` bridge. The popup and content scripts
//! send a base64-encoded [`valet::Request`] as the message body; this
//! module forwards each call through a shared
//! [`Client<NativeMessage>`] to the native host and returns
//! `{ result: <b64>, backend: "native" }` to the caller. Re-entrant
//! calls are safe because `Client<NativeMessage>` multiplexes them
//! over one port.

use std::cell::RefCell;
use std::rc::Rc;

use serde::Serialize;
use valet::protocol::frame::Frame;
use valet::protocol::{Client, NativeMessage};
use valet::{LocalHandler, Request};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::future_to_promise;

use super::externs;

const NATIVE_APP: &str = "com.nixpulvis.valet";

thread_local! {
    static CLIENT: RefCell<Option<Rc<Client<NativeMessage>>>> = const { RefCell::new(None) };
}

fn client() -> Rc<Client<NativeMessage>> {
    CLIENT.with(|cell| {
        if let Some(c) = cell.borrow().as_ref() {
            return c.clone();
        }
        tracing::info!(app = NATIVE_APP, "connecting native host");
        let c = Rc::new(Client::<NativeMessage>::connect(NATIVE_APP));
        *cell.borrow_mut() = Some(c.clone());
        c
    })
}

/// Register the `browser.runtime.onMessage` handler. Each message is a
/// base64 [`valet::Request`]; we hand it to the shared
/// [`Client<NativeMessage>`] and return the base64 [`valet::Response`].
pub fn install_message_listener() {
    let cb = Closure::wrap(Box::new(|msg: JsValue, _sender: JsValue| -> JsValue {
        future_to_promise(async move { handle_rpc(msg).await }).into()
    }) as Box<dyn FnMut(JsValue, JsValue) -> JsValue>);
    externs::runtime_on_message_add_listener(cb.as_ref().unchecked_ref());
    cb.forget();
}

async fn handle_rpc(msg: JsValue) -> Result<JsValue, JsValue> {
    let request_b64 = msg
        .as_string()
        .ok_or_else(|| JsValue::from_str("message must be a base64 string"))?;
    let request = Request::decode_base64(&request_b64)
        .map_err(|e| JsValue::from_str(&format!("decode request: {e}")))?;
    let req_kind: &'static str = (&request).into();
    tracing::trace!(request = req_kind, "→ rpc");

    let response = match client().handle(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "rpc transport failed");
            return Err(JsValue::from_str(&e.to_string()));
        }
    };
    let resp_kind: &'static str = (&response).into();
    tracing::trace!(response = resp_kind, "← rpc");

    #[derive(Serialize)]
    struct Resp {
        result: String,
        backend: &'static str,
    }
    let serializer = serde_wasm_bindgen::Serializer::json_compatible();
    serde::Serialize::serialize(
        &Resp {
            result: response.encode_base64(),
            backend: "native",
        },
        &serializer,
    )
    .map_err(|e| JsValue::from_str(&e.to_string()))
}
