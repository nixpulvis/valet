//! The **Valet** password manager framework.
//!
//! TODO

#[cfg(feature = "db")]
pub mod db;
pub mod encrypt;
#[cfg(feature = "ffi")]
pub mod ffi;
#[cfg(feature = "logging")]
pub mod logging;
pub mod lot;
pub mod password;
pub mod prelude;
pub mod protocol;
pub mod record;
pub mod user;
pub mod uuid;

#[cfg(any(
    feature = "protocol-embedded",
    feature = "protocol-socket",
    feature = "protocol-native-msg-client",
))]
pub use self::protocol::Client;
#[cfg(any(feature = "protocol-socket", feature = "protocol-native-msg-server"))]
pub use self::protocol::Server;
pub use self::protocol::message::{Request, Response};
pub use self::protocol::{Handler, LocalHandler};

// Some top-level re-exports for the most important structures in valet. Mostly
// for visibility in the docs, developers will likely use the prelude.
pub use self::lot::Lot;
pub use self::record::Record;
pub use self::user::User;
