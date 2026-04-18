//! Wire protocol and transport client for the Valet password manager.
//!
//! Two consumers share this crate:
//!
//! * Rust clients and the future daemon depend on it as an `rlib` and speak
//!   [`Request`]/[`Response`] over a [`std::os::unix::net::UnixStream`] via
//!   [`Request::read`]/[`Request::write`] and the matching [`Response`] pair.
//! * The macOS AutoFill extension links against it as a `staticlib` through
//!   the extern-"C" surface in [`ffi`]. The C header is regenerated at build
//!   time into `include/valet_ipc.h`.
//!
//! The wire payload types are the same [`valet::Record`] / [`valet::record::Data`]
//! / [`valet::password::Password`] / [`valet::uuid::Uuid`] types used inside
//! the core library — there are no parallel DTOs.

pub mod client;
pub mod request;

#[cfg(feature = "stub")]
pub mod stub;

pub mod ffi;

pub use request::{Request, Response};
