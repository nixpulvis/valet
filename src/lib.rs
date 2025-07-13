//! The **Valet** password manager framework.
//!
//! TODO

use crate::user::Credential;

#[cfg(feature = "gui")]
mod gui;
pub mod lot;
pub mod prelude;
pub mod record;
pub mod user;
