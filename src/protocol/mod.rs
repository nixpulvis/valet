//! Handler trait, `Call`-based dispatch, and per-protocol client/server types.
//!
//! Three things live here:
//!
//! * [`Handler`] / [`SendHandler`]: "turns a [`Request`] into a
//!   [`Response`]". Every concrete per-protocol client implements one
//!   of these. [`Handler`] is the base; [`SendHandler`] is the
//!   `Send + Sync` refinement used by native (tokio) callers. The
//!   uniform typed-call entry points are [`Handler::call`] and
//!   [`SendHandler::call`]: callers build a payload struct (e.g.
//!   [`message::Unlock`]) that implements [`message::Call`] and hand
//!   it to `call`.
//! * [`Serve`]: "runs a server loop that forwards every incoming
//!   request to a [`SendHandler`]". Implemented by [`SocketServer`]
//!   and [`NativeMessageServer`].
//! * The concrete per-protocol types ([`EmbeddedHandler`],
//!   [`SocketClient`] / [`SocketServer`], [`NativeMessageClient`] /
//!   [`NativeMessageServer`]). Each is feature-gated; a build without
//!   the matching `protocol-*` feature doesn't see the type.
//!
//! The in-process "embedded" case is a [`Handler`] that owns a SQLite
//! handle and dispatches requests directly. It is not a "protocol";
//! nothing frames bytes on the wire.
//!
//! [`Request`]: message::Request
//! [`Response`]: message::Response

use crate::protocol::message::{Call, Request, Response};
use std::io;

pub mod frame;
pub mod message;

mod impls;

#[cfg(all(
    feature = "ffi",
    any(feature = "protocol-embedded", feature = "protocol-socket"),
))]
pub mod ffi;

// Flat re-export of each concrete protocol's module so consumers can
// keep writing `valet::protocol::embedded::...`, `valet::protocol::
// socket::...`, etc. The on-disk grouping under `impls/` is an
// implementation detail.
#[cfg(feature = "protocol-embedded")]
pub use impls::embedded;
#[cfg(any(
    feature = "protocol-native-msg-server",
    feature = "protocol-native-msg-client",
))]
pub use impls::native_msg;
#[cfg(feature = "protocol-socket")]
pub use impls::socket;

// Short re-exports so callers can name the concrete handler/server
// types without threading through their module paths.
#[cfg(feature = "protocol-embedded")]
pub use impls::embedded::EmbeddedHandler;
#[cfg(feature = "protocol-native-msg-client")]
pub use impls::native_msg::NativeMessageClient;
#[cfg(feature = "protocol-native-msg-server")]
pub use impls::native_msg::NativeMessageServer;
#[cfg(feature = "protocol-socket")]
pub use impls::socket::{SocketClient, SocketServer};

/// Anything that can turn a [`Request`] into a [`Response`]. Provider
/// side of every RPC. Base trait with no `Send`/`Sync` requirement;
/// works for the browser WASM build's [`NativeMessageClient`] whose JS
/// `Port` is `!Send`. Native callers want the [`SendHandler`] subtrait
/// instead, which pins down `Send + Sync` on the implementor and
/// `+ Send` on every returned future.
///
/// [`io::Result`] is the outer return for transport failures (socket
/// dropped, disk full while writing, ...); application-level failures
/// (locked user, record not found, bad query) are conveyed as
/// [`Response::Error`]. In-process handlers never return `Err`; `Err`
/// only comes from remote-forwarding clients.
///
/// Typed calls go through [`Handler::call`], which takes any [`Call`]
/// payload struct and extracts its declared response type.
pub trait Handler {
    /// Dispatch one [`Request`] to its [`Response`]. [`Handler::call`]
    /// is the typed wrapper built on top of this.
    fn handle(&self, req: Request) -> impl std::future::Future<Output = io::Result<Response>>;

    /// Issue a typed [`Call`]. Converts the payload into a
    /// [`Request`], dispatches it through [`Handler::handle`], and
    /// extracts the declared [`Call::Response`].
    fn call<C: Call>(&self, c: C) -> impl std::future::Future<Output = Result<C::Response, Error>> {
        async move { Ok(C::from_response(self.handle(c.into_request()).await?)?) }
    }
}

/// `Send + Sync` refinement of [`Handler`]. Every implementor is also
/// a [`Handler`] via the blanket below; the difference is that
/// [`SendHandler::handle`] and [`SendHandler::call`] return
/// [`Send`] futures, which is what `tokio::spawn` and multi-threaded
/// [`Serve`] loops require. Every native handler
/// ([`EmbeddedHandler`], [`SocketClient`], `Arc<H>`) impls this.
pub trait SendHandler: Send + Sync {
    /// Dispatch one [`Request`] to its [`Response`], returning a
    /// [`Send`] future.
    fn handle(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = io::Result<Response>> + Send;

    /// Send-bounded typed [`Call`].
    fn call<C: Call + Send>(
        &self,
        c: C,
    ) -> impl std::future::Future<Output = Result<C::Response, Error>> + Send
    where
        C::Response: Send,
    {
        async move { Ok(C::from_response(self.handle(c.into_request()).await?)?) }
    }
}

// Every `SendHandler` is also a `Handler`; the Send bounds just
// aren't projected. One-way bridge: `Handler` doesn't imply
// `SendHandler` because the futures aren't guaranteed Send.
impl<T: SendHandler + ?Sized> Handler for T {
    fn handle(&self, req: Request) -> impl std::future::Future<Output = io::Result<Response>> {
        <Self as SendHandler>::handle(self, req)
    }
}

// Blanket impl so `Arc<H>` is itself a `SendHandler`. Lets callers
// hand an `Arc<EmbeddedHandler>` to a server's `serve` without a
// wrapper, and lets FFI / daemon / transport layers uniformly work
// with `H: SendHandler`.
impl<H: SendHandler + ?Sized> SendHandler for std::sync::Arc<H> {
    fn handle(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = io::Result<Response>> + Send {
        (**self).handle(req)
    }
}

/// A server-side role that accepts connections and dispatches each
/// incoming [`Request`] through a [`SendHandler`]. Implemented by
/// [`SocketServer`] and [`NativeMessageServer`].
#[cfg(feature = "_protocols")]
pub trait Serve {
    /// Run the accept/dispatch loop, forwarding every request to
    /// `handler`. Never returns under normal operation.
    fn serve<H: SendHandler + 'static>(
        self,
        handler: std::sync::Arc<H>,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
}

/// Top-level error returned by [`Handler::call`] and
/// [`SendHandler::call`]. Collapses transport failures and
/// application-level errors into one shape so callers only need one
/// `?` chain.
#[derive(Debug)]
pub enum Error {
    /// Transport / IO failure (socket dropped, encode/decode failure,
    /// etc.).
    Io(io::Error),
    /// The remote peer's handler returned [`Response::Error`]; the
    /// payload is its message.
    Remote(String),
    /// The remote peer's handler returned a [`Response`] variant that
    /// doesn't match the one the typed call expected. Indicates a
    /// version skew between peers.
    Unexpected,
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<message::ResponseError> for Error {
    fn from(e: message::ResponseError) -> Self {
        match e {
            message::ResponseError::Remote(msg) => Error::Remote(msg),
            message::ResponseError::UnexpectedResponse => Error::Unexpected,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Remote(msg) => write!(f, "remote: {msg}"),
            Error::Unexpected => write!(f, "unexpected response variant"),
        }
    }
}

impl std::error::Error for Error {}
