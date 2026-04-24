//! Integration tests for the `valet::protocol::*` surface. The
//! per-protocol submodules (`embedded`, `socket`, `native_msg`) each
//! exercise one protocol in isolation against a stub or an in-memory
//! DB; the `multi` submodule exercises cross-protocol composition
//! (relays, daemon-as-both-ends). All compiled into a single
//! `protocol` test binary.

#[path = "common/mod.rs"]
mod common;

#[cfg(feature = "protocol-embedded")]
#[path = "protocol/embedded.rs"]
mod embedded;

#[cfg(feature = "protocol-socket")]
#[path = "protocol/socket.rs"]
mod socket;

#[cfg(feature = "protocol-native-msg-server")]
#[path = "protocol/native_msg.rs"]
mod native_msg;

#[cfg(all(
    feature = "protocol-embedded",
    feature = "protocol-socket",
    feature = "protocol-native-msg-server",
))]
#[path = "protocol/multi.rs"]
mod multi;
