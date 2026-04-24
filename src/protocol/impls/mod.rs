//! Concrete `Protocol` implementations. Each submodule defines one
//! wire format and implements [`Client<P>`] (and, where applicable,
//! [`Server<P>`]) for its marker type. Publicly re-exported flat at
//! [`crate::protocol`] so consumers write `valet::protocol::socket::
//! Socket` rather than `valet::protocol::impls::socket::Socket`.
//!
//! [`Client<P>`]: crate::protocol::Client
//! [`Server<P>`]: crate::protocol::Server

#[cfg(feature = "protocol-embedded")]
pub mod embedded;

#[cfg(feature = "protocol-socket")]
pub mod socket;

#[cfg(any(
    feature = "protocol-native-msg-server",
    feature = "protocol-native-msg-client",
))]
pub mod native_msg;
