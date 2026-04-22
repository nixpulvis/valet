//! Wire protocols, handler trait, and the typed `Client<P>` / `Server<P>`
//! pair for each protocol.
//!
//! One crate, one `Handler` abstraction, three orthogonal pieces that
//! snap together to make every consumer:
//!
//! * [`Handler`] - "turns a [`Request`] into a [`Response`]". Implemented
//!   by [`Client<Embedded>`] (dispatches against a local DB) and by
//!   [`Client<Socket>`] (forwards to a remote daemon). Anything that
//!   answers requests implements this.
//! * [`Protocol`] - marker trait for a wire format. `Embedded`,
//!   `Socket`, `NativeMessage` implement it; each nominates its own
//!   per-role connection types via the assoc types.
//! * [`Client<P>`] / [`Server<P>`] - the two roles of one wire format.
//!   A `Server<P>` reads requests off `P` and delegates them to any
//!   `Handler`; a `Client<P>` wraps its `P::Client` connection and
//!   implements `Handler` by shipping each request over that
//!   connection. `Embedded` has no server half - it answers calls
//!   directly, nothing listens.
//!
//! The daemon wires a `Server<_>` to a `Handler`; callers hold a
//! `Client<_>` directly. `Server<Socket>` serving `Client<Embedded>`
//! is the usual desktop setup. `Server<NativeMessage>` serving
//! `Client<Socket>` is a pure byte relay (the old
//! `platform/browser/native-host` socket-relay mode).

use crate::{Lot, Record};
use crate::record::Label;
use crate::uuid::Uuid;
use message::RevisionEntry;
#[cfg(feature = "_protocols")]
use std::convert::Infallible;
use std::io;

pub mod frame;
pub mod message;

mod impls;

#[cfg(all(
    feature = "ffi",
    any(feature = "protocol-embedded", feature = "protocol-socket"),
))]
pub mod ffi;

use message::{Request, Response};

// Flat re-export of each concrete protocol's module so consumers
// can keep writing `valet::protocol::embedded::...`,
// `valet::protocol::socket::...`, etc. The on-disk grouping under
// `impls/` is an implementation detail.
#[cfg(feature = "protocol-embedded")]
pub use impls::embedded;
#[cfg(any(
    feature = "protocol-native-msg-server",
    feature = "protocol-native-msg-client",
))]
pub use impls::native_msg;
#[cfg(feature = "protocol-socket")]
pub use impls::socket;

// Short marker re-exports so callers who only need the tag type
// don't reach through the module path.
#[cfg(feature = "protocol-embedded")]
pub use impls::embedded::Embedded;
#[cfg(any(
    feature = "protocol-native-msg-server",
    feature = "protocol-native-msg-client",
))]
pub use impls::native_msg::NativeMessage;
#[cfg(feature = "protocol-socket")]
pub use impls::socket::Socket;

/// Placeholder for ! type. Used for a missing client or server half of a
/// [`Protocol`]. `Client<P>` / `Server<P>` with this inner is uninhabited.
#[cfg(feature = "_protocols")]
pub type Never = Infallible;

/// A wire protocol between a client and a server. The associated types
/// pin down what a [`Client<Self>`] wraps and what a [`Server<Self>`]
/// wraps; they can (and usually do) differ, since a client holds a
/// connected stream while a server holds a listener.
///
/// Protocols with no client half (none today) or no server half
/// ([`Embedded`]) use [`Never`] for the missing side; the
/// corresponding `Client<P>` / `Server<P>` then has no inhabitants
/// and no constructor.
#[cfg(feature = "_protocols")]
pub trait Protocol {
    /// Per-protocol state carried by `Client<Self>`. For wire protocols
    /// this is typically a connected stream behind a mutex; for
    /// [`Embedded`] it is the in-proc DB handle and unlock cache.
    type Client;
    /// Per-protocol state carried by `Server<Self>`. For wire protocols
    /// this is typically a listener; for protocols without a server
    /// half (e.g. [`Embedded`]) this is [`Never`].
    type Server;
}

/// Anything that can turn a [`Request`] into a [`Response`]. Provider
/// side of every RPC. Implemented by:
///
/// * [`Client<Embedded>`] - dispatches against its own SQLite handle.
/// * [`Client<Socket>`] - forwards frames to a remote daemon over a
///   Unix socket, becoming a `Handler` by delegation.
/// * Test fixtures (`StubHandler` in `tests/common/stub.rs`).
///
/// [`io::Result`] is the outer return for transport failures (socket
/// dropped, disk full while writing, ...); application-level failures
/// (locked user, record not found, bad query) are conveyed as
/// [`Response::Error`]. In-process handlers never return `Err`; `Err`
/// only comes from remote-forwarding `Client`s.
///
/// The typed method surface (`status`, `unlock`, `list`, ...) is
/// provided here as default methods on top of [`handle`](Self::handle).
/// Every implementor inherits them; callers hold a `Client<P>`, a
/// `StubHandler`, or any other `impl Handler` and use the typed
/// methods uniformly.
///
/// Two trait variants are declared side-by-side:
///
/// * [`Handler`] requires `Send + Sync` on the implementor and
///   `+ Send` on every returned future. Anywhere that needs to
///   `tokio::spawn` a handler future (`Server<Socket>::serve`, test
///   fixtures) asks for `H: Handler`. Every native handler
///   (`Client<Embedded>`, `Client<Socket>`, `Arc<H>`) impls this.
/// * [`LocalHandler`] is the unbounded version. The browser WASM
///   build's `Client<NativeMessage>` impls it directly because the JS
///   `Port` it wraps is `!Send`. A blanket impl makes every `Handler`
///   a `LocalHandler`, so callers that only need one trait can target
///   `LocalHandler` and still accept both.
/// Expands the full typed handler method surface inside a trait body.
/// The single token argument `$($bound:tt)*` is appended to every
/// returned `impl Future` so the same source emits a `+ Send` version
/// for [`Handler`] and an unbounded version for [`LocalHandler`]. The
/// abstract `handle` is declared outside the macro because its Send
/// bound is the only place the two traits diverge on the required
/// method, and keeping it separate makes the macro purely about
/// defaulted wrappers.
macro_rules! handler_methods {
    ($($bound:tt)*) => {
        /// Currently-unlocked usernames. Mirrors [`Request::Status`].
        fn status(
            &self,
        ) -> impl std::future::Future<Output = Result<Vec<String>, Error>> $($bound)* {
            async move { Ok(self.handle(Request::Status).await?.expect_users()?) }
        }

        /// Every registered username. Mirrors [`Request::ListUsers`].
        fn list_users(
            &self,
        ) -> impl std::future::Future<Output = Result<Vec<String>, Error>> $($bound)* {
            async move { Ok(self.handle(Request::ListUsers).await?.expect_users()?) }
        }

        /// Derive the user's key and cache it. Mirrors [`Request::Unlock`].
        fn unlock(
            &self,
            username: String,
            password: crate::password::Password,
        ) -> impl std::future::Future<Output = Result<(), Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::Unlock { username, password })
                    .await?
                    .expect_ok()?)
            }
        }

        /// Drop the cached keys for one user. Mirrors [`Request::Lock`].
        fn lock(
            &self,
            username: String,
        ) -> impl std::future::Future<Output = Result<(), Error>> $($bound)* {
            async move {
                Ok(self.handle(Request::Lock { username }).await?.expect_ok()?)
            }
        }

        /// Drop every cached user. Mirrors [`Request::LockAll`].
        fn lock_all(
            &self,
        ) -> impl std::future::Future<Output = Result<(), Error>> $($bound)* {
            async move { Ok(self.handle(Request::LockAll).await?.expect_ok()?) }
        }

        /// Cross-lot query-language search. Mirrors [`Request::List`].
        fn list(
            &self,
            username: String,
            queries: Vec<String>,
        ) -> impl std::future::Future<Output = Result<Vec<(Uuid<Record>, Label)>, Error>>
               $($bound)* {
            async move {
                Ok(self
                    .handle(Request::List { username, queries })
                    .await?
                    .expect_index()?)
            }
        }

        /// Fetch a decrypted record by uuid. Mirrors [`Request::Fetch`].
        fn fetch(
            &self,
            username: String,
            uuid: Uuid<Record>,
        ) -> impl std::future::Future<Output = Result<Record, Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::Fetch { username, uuid })
                    .await?
                    .expect_record()?)
            }
        }

        /// Per-lot domain match. Mirrors [`Request::FindRecords`].
        fn find_records(
            &self,
            username: String,
            lot: String,
            query: String,
        ) -> impl std::future::Future<Output = Result<Vec<(Uuid<Record>, Label)>, Error>>
               $($bound)* {
            async move {
                Ok(self
                    .handle(Request::FindRecords {
                        username,
                        lot,
                        query,
                    })
                    .await?
                    .expect_index()?)
            }
        }

        /// Fetch a decrypted record by uuid in a specific lot. Mirrors
        /// [`Request::GetRecord`].
        fn get_record(
            &self,
            username: String,
            lot: String,
            uuid: Uuid<Record>,
        ) -> impl std::future::Future<Output = Result<Record, Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::GetRecord {
                        username,
                        lot,
                        uuid,
                    })
                    .await?
                    .expect_record()?)
            }
        }

        /// Create a new record with a caller-supplied password. Mirrors
        /// [`Request::CreateRecord`].
        fn create_record(
            &self,
            username: String,
            lot: String,
            label: Label,
            password: crate::password::Password,
            extra: std::collections::HashMap<String, String>,
        ) -> impl std::future::Future<Output = Result<Record, Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::CreateRecord {
                        username,
                        lot,
                        label,
                        password,
                        extra,
                    })
                    .await?
                    .expect_record()?)
            }
        }

        /// Create a new record with a generated password. Mirrors
        /// [`Request::GenerateRecord`].
        fn generate_record(
            &self,
            username: String,
            lot: String,
            label: Label,
        ) -> impl std::future::Future<Output = Result<Record, Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::GenerateRecord {
                        username,
                        lot,
                        label,
                    })
                    .await?
                    .expect_record()?)
            }
        }

        /// Register a new user and their default lot. Mirrors
        /// [`Request::Register`].
        fn register(
            &self,
            username: String,
            password: crate::password::Password,
        ) -> impl std::future::Future<Output = Result<(), Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::Register { username, password })
                    .await?
                    .expect_ok()?)
            }
        }

        /// Verify a password without caching. Mirrors [`Request::Validate`].
        fn validate(
            &self,
            username: String,
            password: crate::password::Password,
        ) -> impl std::future::Future<Output = Result<(), Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::Validate { username, password })
                    .await?
                    .expect_ok()?)
            }
        }

        /// List every lot the user has access to. Mirrors
        /// [`Request::ListLots`].
        fn list_lots(
            &self,
            username: String,
        ) -> impl std::future::Future<Output = Result<Vec<(Uuid<Lot>, String)>, Error>>
               $($bound)* {
            async move {
                Ok(self
                    .handle(Request::ListLots { username })
                    .await?
                    .expect_lots()?)
            }
        }

        /// Create a new lot. Mirrors [`Request::CreateLot`].
        fn create_lot(
            &self,
            username: String,
            lot: String,
        ) -> impl std::future::Future<Output = Result<(), Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::CreateLot { username, lot })
                    .await?
                    .expect_ok()?)
            }
        }

        /// Delete a lot. Mirrors [`Request::DeleteLot`].
        fn delete_lot(
            &self,
            username: String,
            lot: String,
        ) -> impl std::future::Future<Output = Result<(), Error>> $($bound)* {
            async move {
                Ok(self
                    .handle(Request::DeleteLot { username, lot })
                    .await?
                    .expect_ok()?)
            }
        }

        /// Walk a record's revisions. Mirrors [`Request::History`].
        fn history(
            &self,
            username: String,
            lot: String,
            uuid: Uuid<Record>,
        ) -> impl std::future::Future<Output = Result<Vec<RevisionEntry>, Error>>
               $($bound)* {
            async move {
                Ok(self
                    .handle(Request::History {
                        username,
                        lot,
                        uuid,
                    })
                    .await?
                    .expect_history()?)
            }
        }
    };
}

pub trait Handler: Send + Sync {
    /// Dispatch one [`Request`] to its [`Response`]. Every typed
    /// method below is a wrapper over this.
    fn handle(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = io::Result<Response>> + Send;

    handler_methods!(+ Send);
}

/// `!Send` superset of [`Handler`]. Same typed method surface, but
/// `handle` (and every default method) returns a non-`Send` future.
/// Implemented by [`Client<NativeMessage>`] in the browser WASM build
/// where the JS `Port` behind the client is `!Send`. Every `Handler`
/// is a `LocalHandler` via the blanket below, so daemon/FFI code that
/// ships `Arc<Client<Embedded>>` still works through the same typed
/// method names on either trait.
pub trait LocalHandler {
    /// Dispatch one [`Request`] to its [`Response`].
    fn handle(&self, req: Request) -> impl std::future::Future<Output = io::Result<Response>>;

    handler_methods!();
}

// Every `Handler` is also a `LocalHandler`; the Send bounds just
// aren't projected. One-way bridge: `LocalHandler` doesn't imply
// `Handler` because the futures aren't guaranteed Send.
impl<T: Handler + ?Sized> LocalHandler for T {
    fn handle(&self, req: Request) -> impl std::future::Future<Output = io::Result<Response>> {
        <Self as Handler>::handle(self, req)
    }
}

// Blanket impl so `Arc<H>` is itself a `Handler`. Lets callers hand an
// `Arc<Client<Embedded>>` to `Server::serve` without a wrapper, and
// lets every FFI / daemon / transport layer uniformly work with
// `H: Handler`.
impl<H: Handler + ?Sized> Handler for std::sync::Arc<H> {
    fn handle(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = io::Result<Response>> + Send {
        (**self).handle(req)
    }
}

/// Typed client over protocol `P`. Constructors, the `Handler` impl,
/// and the per-operation typed method surface (`status`, `unlock`,
/// `list`, ...) all live in the per-protocol module. Only protocols
/// with a client half compile this; a build with just
/// `protocol-native-msg-server` on has no `Client<P>` (and no need for one).
#[cfg(any(
    feature = "protocol-embedded",
    feature = "protocol-socket",
    feature = "protocol-native-msg-client",
))]
pub struct Client<P: Protocol> {
    inner: P::Client,
}

/// Typed server over protocol `P`. Constructors and the `serve`
/// method live in the per-protocol module. Only wire protocols have
/// server halves, so this is gated on those specifically; `Server
/// <Embedded>` would be uninhabited anyway.
#[cfg(any(feature = "protocol-socket", feature = "protocol-native-msg-server"))]
pub struct Server<P: Protocol> {
    inner: P::Server,
}

/// Top-level error returned by the typed [`Client<P>`] method surface.
/// Collapses transport failures and application-level errors into one
/// shape so callers only need one `?` chain.
#[derive(Debug)]
pub enum Error {
    /// Transport / IO failure (socket dropped, encode/decode failure,
    /// etc.).
    Io(io::Error),
    /// The remote peer's handler returned [`Response::Error`]; the
    /// payload is its message.
    Remote(String),
    /// The remote peer's handler returned a [`Response`] variant that
    /// doesn't match the one the typed method expected. Indicates a
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
