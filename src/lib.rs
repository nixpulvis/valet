//! The **Valet** password manager framework.
//!
//! TODO

#[derive(Debug, PartialEq, Eq)]
pub struct Uuid<T>(uuid::Uuid, PhantomData<T>);

impl<T> Uuid<T> {
    pub fn now() -> Self {
        Uuid(uuid::Uuid::now_v7(), PhantomData)
    }

    pub fn parse(s: &str) -> Result<Self, Error> {
        Ok(Uuid(uuid::Uuid::parse_str(s)?, PhantomData))
    }
}

impl<T> Clone for Uuid<T> {
    fn clone(&self) -> Self {
        Uuid(self.0.clone(), PhantomData)
    }
}

impl<T> Deref for Uuid<T> {
    type Target = uuid::Uuid;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
pub enum Error {
    Uuid(uuid::Error),
}

impl From<uuid::Error> for Error {
    fn from(err: uuid::Error) -> Self {
        Error::Uuid(err)
    }
}

pub mod db;
pub mod encrypt;
pub mod lot;
pub mod prelude;
pub mod record;
pub mod user;

use std::marker::PhantomData;
use std::ops::Deref;

// Some toplevel re-exports for the most important structures in valet. Mostly
// for visability in the docs, developers will likely use the prelude.
pub use self::lot::Lot;
pub use self::record::Record;
pub use self::user::User;
