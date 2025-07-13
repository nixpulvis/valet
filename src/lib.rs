//! The **Valet** password manager framework.
//!
//! TODO

use crate::user::Credential;

pub mod database;
#[cfg(feature = "gui")]
mod gui;
pub mod lot;
pub mod prelude;
pub mod user;
