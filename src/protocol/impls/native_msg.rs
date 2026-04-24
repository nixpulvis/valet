//! Native-messaging protocol.
//!
//! The WebExtensions native-messaging JSON envelope shape is used by
//! both ends of the browser <-> daemon connection:
//!
//! * On the browser side, [`Client<NativeMessage>`] wraps a
//!   `browser.runtime.Port` returned by `connectNative` and multiplexes
//!   RPC calls over it. It impls [`LocalHandler`] (non-`Send`; the JS
//!   port is `!Send`) so extension code can use the same typed method
//!   surface as native callers of [`Client<Embedded>`] or
//!   [`Client<Socket>`]. Enabled by `protocol-native-msg-client`.
//! * On the daemon side, [`Server<NativeMessage>`] parses
//!   [`NativeRequest`]s off stdin, dispatches through a [`Handler`],
//!   and writes [`NativeReply`]s back to stdout. Enabled by
//!   `protocol-native-msg-server`, which pulls in tokio.
//!
//! The envelope types ([`NativeRequest`], [`NativeReply`],
//! [`NativePayload`]) are always compiled so the two halves share one
//! definition regardless of which feature flags are set.
//!
//! Envelope shape:
//!
//! ```text
//! in:  { "id": <n>, "request": "<base64-bitcode-Request>" }
//! out: { "backend": "<name>", "payload": Ok({ "id": <n>, "data": "<base64>" })
//!                          | Err("<msg>") }
//! ```
//!
//! [`Handler`]: crate::protocol::Handler
//! [`LocalHandler`]: crate::protocol::LocalHandler
//! [`Client<NativeMessage>`]: crate::protocol::Client
//! [`Client<Embedded>`]: crate::protocol::Client
//! [`Client<Socket>`]: crate::protocol::Client
//! [`Server<NativeMessage>`]: crate::protocol::Server

use serde::{Deserialize, Serialize};

/// Monotonically increasing correlation id for one native-messaging
/// round trip. Chosen by the client, echoed back in the matching
/// [`NativePayload`] so out-of-order replies can be routed.
pub type NativeId = u64;

/// Request envelope posted from the browser to the native-messaging
/// server. `request` is a base64-encoded bitcode [`Request`]; the
/// server decodes it, dispatches through its handler, and replies
/// with a [`NativeReply`] carrying the base64-encoded [`Response`].
/// `id` correlates the reply back to the pending caller on the
/// browser side.
///
/// [`Request`]: crate::protocol::message::Request
/// [`Response`]: crate::protocol::message::Response
#[derive(Serialize, Deserialize)]
pub struct NativeRequest {
    pub id: NativeId,
    pub request: String,
}

/// Reply envelope returned to the browser. `backend` is the
/// transport that actually served the call (`"socket"` or
/// `"embedded"`). `payload` carries the base64-encoded bitcode
/// [`Response`] on success (with the request id echoed back) or an
/// error message on failure.
///
/// [`Response`]: crate::protocol::message::Response
#[derive(Serialize, Deserialize)]
pub struct NativeReply {
    pub backend: String,
    pub payload: Result<NativePayload, String>,
}

/// Success body inside [`NativeReply::payload`].
#[derive(Serialize, Deserialize)]
pub struct NativePayload {
    pub id: NativeId,
    pub data: String,
}

/// Maximum native-messaging JSON envelope size (1 MiB). Matches the
/// limit Firefox and Chrome enforce on the browser side; frames larger
/// than this cause the browser to drop the port.
pub const MAX_SIZE: usize = 1024 * 1024;

#[cfg(any(
    feature = "protocol-native-msg-server",
    feature = "protocol-native-msg-client"
))]
pub use marker::NativeMessage;

#[cfg(feature = "protocol-native-msg-server")]
pub use server::{NativeMessageServer, serve_io};

#[cfg(feature = "protocol-native-msg-client")]
pub use client::NativeMessageClient;

#[cfg(any(
    feature = "protocol-native-msg-server",
    feature = "protocol-native-msg-client"
))]
mod marker {
    #[cfg(any(
        not(feature = "protocol-native-msg-client"),
        not(feature = "protocol-native-msg-server"),
    ))]
    use crate::protocol::Never;
    use crate::protocol::Protocol;

    /// Wire-protocol marker for WebExtensions native messaging.
    pub struct NativeMessage;

    impl Protocol for NativeMessage {
        #[cfg(feature = "protocol-native-msg-client")]
        type Client = super::client::NativeMessageClient;
        #[cfg(not(feature = "protocol-native-msg-client"))]
        type Client = Never;

        #[cfg(feature = "protocol-native-msg-server")]
        type Server = super::server::NativeMessageServer;
        #[cfg(not(feature = "protocol-native-msg-server"))]
        type Server = Never;
    }
}

#[cfg(feature = "protocol-native-msg-server")]
mod server {
    use super::{MAX_SIZE, NativeMessage, NativePayload, NativeReply, NativeRequest};
    use crate::protocol::Handler;
    use crate::protocol::frame::Frame;
    use crate::protocol::message::Request;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::io;
    use std::sync::Arc;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tracing::{debug, error, warn};

    /// State behind [`crate::protocol::Server<NativeMessage>`]. Owns
    /// stdin/stdout; there is at most one `Server<NativeMessage>` per
    /// process because the browser funnels one native-messaging
    /// session through the standard streams.
    pub struct NativeMessageServer {
        pub(crate) backend_name: &'static str,
    }

    impl crate::protocol::Server<NativeMessage> {
        /// Build a native-messaging server bound to this process's
        /// stdin/stdout. `backend_name` is the short tag echoed back
        /// in every reply so the browser side can log which transport
        /// served each call (`"embedded"` vs `"socket"`).
        pub fn from_stdio(backend_name: &'static str) -> Self {
            crate::protocol::Server {
                inner: NativeMessageServer { backend_name },
            }
        }

        /// Run the stdio loop. Reads one envelope at a time from
        /// stdin, dispatches it through `handler`, writes one
        /// envelope to stdout. Returns `Ok(())` on clean stdin EOF
        /// (browser closed the pipe).
        pub async fn serve<H: Handler>(self, handler: Arc<H>) -> io::Result<()> {
            serve_io(
                self.inner.backend_name,
                tokio::io::stdin(),
                tokio::io::stdout(),
                handler,
            )
            .await
        }
    }

    /// Run the native-messaging dispatch loop on arbitrary async
    /// byte streams. [`crate::protocol::Server::<NativeMessage>::
    /// serve`] is a thin wrapper that passes `tokio::io::stdin` /
    /// `stdout`; this split exists so tests can drive the loop with
    /// [`tokio::io::duplex`] pipes, and so future deployments could
    /// funnel native-messaging frames over any other bidirectional
    /// transport (an SSH channel, a Unix pipe pair) without stdio.
    pub async fn serve_io<R, W, H>(
        backend: &'static str,
        mut reader: R,
        mut writer: W,
        handler: Arc<H>,
    ) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
        H: Handler,
    {
        loop {
            let mut len_buf = [0u8; 4];
            match reader.read_exact(&mut len_buf).await {
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    debug!("stdin closed, exiting");
                    return Ok(());
                }
                Err(e) => {
                    warn!("stdin read failed, exiting: {e}");
                    return Err(e);
                }
            }
            let len = u32::from_le_bytes(len_buf) as usize;
            if len == 0 || len > MAX_SIZE {
                warn!(len, "invalid frame length, exiting");
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid native-messaging frame length",
                ));
            }
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).await?;

            let reply = match serde_json::from_slice::<NativeRequest>(&buf) {
                Ok(req) => handle_envelope(&*handler, backend, req).await,
                Err(e) => {
                    warn!(backend, "invalid json from browser: {e}");
                    NativeReply {
                        backend: backend.to_owned(),
                        payload: Err(format!("invalid json: {e}")),
                    }
                }
            };

            let bytes = match serde_json::to_vec(&reply) {
                Ok(b) => b,
                Err(e) => {
                    warn!("failed to serialize reply: {e}");
                    continue;
                }
            };
            if bytes.len() > MAX_SIZE {
                warn!(bytes = bytes.len(), "reply too large");
                continue;
            }
            let header = (bytes.len() as u32).to_le_bytes();
            writer.write_all(&header).await?;
            writer.write_all(&bytes).await?;
            writer.flush().await?;
        }
    }

    async fn handle_envelope<H: Handler>(
        handler: &H,
        backend: &'static str,
        req: NativeRequest,
    ) -> NativeReply {
        let request_bytes = match STANDARD.decode(&req.request) {
            Ok(b) => b,
            Err(e) => return native_err(backend, format!("invalid base64: {e}")),
        };
        let request = match Request::decode(&request_bytes) {
            Ok(r) => r,
            Err(e) => return native_err(backend, format!("decode: {e}")),
        };
        match handler.handle(request).await {
            Ok(response) => NativeReply {
                backend: backend.to_owned(),
                payload: Ok(NativePayload {
                    id: req.id,
                    data: STANDARD.encode(response.encode()),
                }),
            },
            Err(e) => native_err(backend, format!("handler: {e}")),
        }
    }

    fn native_err(backend: &'static str, msg: String) -> NativeReply {
        error!(backend, "{msg}");
        NativeReply {
            backend: backend.to_owned(),
            payload: Err(msg),
        }
    }
}

#[cfg(feature = "protocol-native-msg-client")]
mod client {
    //! WASM-side `Client<NativeMessage>`: wraps a
    //! `browser.runtime.Port` returned by `connectNative`, multiplexes
    //! RPC calls over it with an id-keyed pending map, and exposes the
    //! result through the [`LocalHandler`] trait so the browser
    //! extension popup can call typed methods (`status`, `list_users`,
    //! ...) the same way a native `Client<Socket>` caller does.
    //!
    //! Wasm is single-threaded, so `Rc<RefCell<...>>` is the right
    //! shared-mutable-state primitive. The client is `!Send` / `!Sync`
    //! (the JS `Port` is neither), which is why it impls
    //! [`LocalHandler`] rather than [`Handler`].
    //!
    //! [`LocalHandler`]: crate::protocol::LocalHandler
    //! [`Handler`]: crate::protocol::Handler
    use super::{NativeId, NativeMessage, NativeReply, NativeRequest};
    use crate::protocol::Client;
    use crate::protocol::frame::Frame;
    use crate::protocol::message::{Request, Response};
    use futures::channel::oneshot;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::io;
    use std::rc::Rc;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// A `browser.runtime.Port` (the value returned by
        /// `connectNative`).
        pub type Port;

        #[wasm_bindgen(method, js_name = postMessage)]
        fn post_message(this: &Port, msg: &JsValue);

        #[wasm_bindgen(method, getter, js_name = onMessage)]
        fn on_message(this: &Port) -> EventTarget;

        #[wasm_bindgen(method, getter, js_name = onDisconnect)]
        fn on_disconnect(this: &Port) -> EventTarget;

        pub type EventTarget;

        #[wasm_bindgen(method, js_name = addListener)]
        fn add_listener(this: &EventTarget, cb: &::js_sys::Function);

        #[wasm_bindgen(js_namespace = ["browser", "runtime"], js_name = connectNative)]
        fn connect_native(name: &str) -> Port;
    }

    /// State behind [`Client<NativeMessage>`]. Holds the live JS
    /// `Port`, the pending-reply map, and the id counter. Single-
    /// threaded; `Rc<RefCell<_>>` is enough.
    pub struct NativeMessageClient {
        port: Port,
        pending: Rc<RefCell<HashMap<NativeId, oneshot::Sender<NativeResult>>>>,
        next_id: Rc<Cell<NativeId>>,
        // Keep the JS closures alive for the lifetime of the client;
        // dropping them would unregister the listeners.
        _on_message: Closure<dyn FnMut(JsValue)>,
        _on_disconnect: Closure<dyn FnMut(JsValue)>,
    }

    type NativeResult = Result<NativeReply, String>;

    impl Client<NativeMessage> {
        /// Open a native-messaging port to the host registered under
        /// `app_name` (the `name` field of the host manifest, e.g.
        /// `"com.nixpulvis.valet"`). Attaches `onMessage` /
        /// `onDisconnect` listeners before returning so no reply is
        /// dropped.
        pub fn connect(app_name: &str) -> Self {
            let port = connect_native(app_name);
            let pending: Rc<RefCell<HashMap<NativeId, oneshot::Sender<NativeResult>>>> =
                Rc::new(RefCell::new(HashMap::new()));
            let next_id = Rc::new(Cell::<NativeId>::new(1));

            let on_message = {
                let pending = pending.clone();
                Closure::wrap(Box::new(move |msg: JsValue| {
                    let reply: NativeReply = match serde_wasm_bindgen::from_value(msg) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!("native reply malformed: {e}");
                            return;
                        }
                    };
                    // Only success payloads carry an id; errors
                    // without one can't be routed. Log and drop.
                    let id = match &reply.payload {
                        Ok(p) => p.id,
                        Err(e) => {
                            tracing::error!(backend = %reply.backend, error = %e, "native reply without id");
                            return;
                        }
                    };
                    match pending.borrow_mut().remove(&id) {
                        Some(tx) => {
                            let _ = tx.send(Ok(reply));
                        }
                        None => tracing::error!(
                            id,
                            backend = %reply.backend,
                            "native reply for unknown id",
                        ),
                    }
                }) as Box<dyn FnMut(JsValue)>)
            };
            port.on_message()
                .add_listener(on_message.as_ref().unchecked_ref());

            let on_disconnect = {
                let pending = pending.clone();
                Closure::wrap(Box::new(move |_p: JsValue| {
                    tracing::warn!("native port disconnected");
                    for (_id, tx) in pending.borrow_mut().drain() {
                        let _ = tx.send(Err("native host disconnected".into()));
                    }
                }) as Box<dyn FnMut(JsValue)>)
            };
            port.on_disconnect()
                .add_listener(on_disconnect.as_ref().unchecked_ref());

            Client {
                inner: NativeMessageClient {
                    port,
                    pending,
                    next_id,
                    _on_message: on_message,
                    _on_disconnect: on_disconnect,
                },
            }
        }
    }

    impl crate::protocol::LocalHandler for Client<NativeMessage> {
        fn handle(&self, req: Request) -> impl std::future::Future<Output = io::Result<Response>> {
            let id = {
                let cur = self.inner.next_id.get();
                let next = cur.wrapping_add(1);
                assert!(next != 0, "native_msg id counter wrapped at u64::MAX");
                self.inner.next_id.set(next);
                cur
            };
            let (tx, rx) = oneshot::channel();
            self.inner.pending.borrow_mut().insert(id, tx);

            let envelope = NativeRequest {
                id,
                request: req.encode_base64(),
            };
            let serializer = serde_wasm_bindgen::Serializer::json_compatible();
            let post_result = serde::Serialize::serialize(&envelope, &serializer)
                .map(|frame| self.inner.port.post_message(&frame));
            let pending = self.inner.pending.clone();

            async move {
                if let Err(e) = post_result {
                    pending.borrow_mut().remove(&id);
                    return Err(io::Error::other(format!("serialize envelope: {e}")));
                }
                let reply = rx
                    .await
                    .map_err(|_| io::Error::other("native port closed"))?
                    .map_err(io::Error::other)?;
                let data = match reply.payload {
                    Ok(p) => p.data,
                    Err(e) => return Err(io::Error::other(e)),
                };
                Response::decode_base64(&data)
                    .map_err(|e| io::Error::other(format!("decode reply: {e}")))
            }
        }
    }
}
