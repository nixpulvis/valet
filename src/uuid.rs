use bitcode::{Decode, Encode};
use core::fmt;
use std::marker::PhantomData;

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
pub struct Uuid<T>([u8; 16], #[bitcode(skip)] PhantomData<T>);

impl<T> Uuid<T> {
    pub fn now() -> Self {
        Uuid(*uuid::Uuid::now_v7().as_bytes(), PhantomData)
    }

    pub fn parse(s: &str) -> Result<Self, Error> {
        let u = uuid::Uuid::parse_str(s)?;
        Ok(Uuid(*u.as_bytes(), PhantomData))
    }

    /// Returns the underlying `uuid::Uuid`.
    pub fn to_uuid(&self) -> uuid::Uuid {
        uuid::Uuid::from_bytes(self.0)
    }
}

impl<T> fmt::Display for Uuid<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_uuid())
    }
}

impl<T> Clone for Uuid<T> {
    fn clone(&self) -> Self {
        Uuid(self.0, PhantomData)
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
