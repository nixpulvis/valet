//! The **Valet** password manager framework.
//!
//! TODO

#[cfg(feature = "db")]
pub mod db;
pub mod encrypt;
#[cfg(feature = "ffi")]
pub mod ffi;
pub mod lot;
pub mod password;
pub mod prelude;
pub mod record;
pub mod user;
pub mod uuid;

// Some top-level re-exports for the most important structures in valet. Mostly
// for visibility in the docs, developers will likely use the prelude.
pub use self::lot::Lot;
pub use self::record::Record;
pub use self::user::User;
