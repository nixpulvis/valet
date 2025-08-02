//! The **Valet** password manager framework.
//!
//! TODO

pub mod db;
pub mod encrypt;
pub mod lot;
pub mod prelude;
pub mod record;
pub mod user;

// Some toplevel re-exports for the most important structures in valet. Mostly
// for visability in the docs, developers will likely use the prelude.
pub use self::lot::Lot;
pub use self::record::Record;
pub use self::user::User;
