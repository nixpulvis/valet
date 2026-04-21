//! Wire protocol, socket-path resolution, and (with the `native` feature)
//! the request handler, remote client, FFI layer, and daemon binary for
//! Valet.
//!
//! All server logic sits behind one abstraction, [`Handler`]: an async
//! `handle(req: Request) -> io::Result<Response>`. Transports call it and
//! don't care whether the backend is the real DB-backed daemon
//! ([`DaemonHandler`]), an in-process fake ([`stub::Stub`]), or a remote
//! socket ([`client::Client`]).
//!
//! The crate has three consumers:
//!
//! * The `valetd` binary (`src/bin/valetd.rs`) wraps a [`DaemonHandler`]
//!   in a Unix-socket listener plus an idle reaper.
//! * Rust clients link this crate as an `rlib` and use [`client::Client`]
//!   — which itself implements [`Handler`] — to speak to the daemon.
//! * The macOS AutoFill extension links this crate as a `staticlib`
//!   through the extern-"C" surface in [`ffi`]; its C header is
//!   regenerated at build time into `target/<triple>/include/valetd.h`.
//!   The FFI picks the concrete [`Handler`] at compile time: [`client::Client`]
//!   by default, or [`stub::Stub`] with `--features stub`.
//!
//! The wire payload types are the same [`valet::Record`] / [`valet::record::Data`]
//! / [`valet::password::Password`] / [`valet::uuid::Uuid`] types used inside
//! the core library; there are no parallel DTOs.
//!
//! With `default-features = false` only [`request`] (minus its tokio async
//! helpers) and [`socket`] compile. The browser extension depends on the
//! crate that way to share one wire schema between the WASM popup and the
//! stdio shim.

pub mod request;
pub mod socket;

#[cfg(feature = "native")]
pub mod client;

#[cfg(feature = "native")]
pub mod server;

#[cfg(feature = "stub")]
pub mod stub;

#[cfg(feature = "native")]
pub mod ffi;

pub use request::{Request, Response};

#[cfg(feature = "native")]
pub use server::{DaemonHandler, Handler};
