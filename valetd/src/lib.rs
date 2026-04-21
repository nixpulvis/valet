//! Wire protocol, socket-path resolution, and (with the `native` feature)
//! the sync client, FFI layer, and daemon binary for Valet.
//!
//! The crate has three consumers:
//!
//! * The `valetd` binary (`src/bin/valetd.rs`) is the daemon itself. It owns
//!   the database, holds unlocked [`valet::user::User`] / [`valet::Lot`]
//!   state, and serves the [`Request`] / [`Response`] protocol on a Unix
//!   socket.
//! * Rust clients link this crate as an `rlib` and use [`client::Client`] to
//!   speak to the daemon synchronously.
//! * The macOS AutoFill extension links this crate as a `staticlib` through
//!   the extern-"C" surface in [`ffi`]; its C header is regenerated at build
//!   time into `target/<triple>/include/valetd.h`.
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

#[cfg(feature = "stub")]
pub mod stub;

#[cfg(feature = "native")]
pub mod ffi;

pub use request::{Request, Response};
