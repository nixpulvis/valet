use bitcode::{Decode, Encode};
use core::fmt;
use std::marker::PhantomData;

/// `PhantomData<fn() -> T>` rather than `PhantomData<T>` so `Uuid<T>`
/// is unconditionally `Send + Sync` - the phantom is only a type tag,
/// we never own a `T`, and we don't want `T`'s auto-traits leaking
/// here (e.g. `Uuid<Lot>` would otherwise inherit the `!Sync` that
/// comes from `Lot`'s live storgit store). Variance in `T` stays
/// covariant, matching the previous `PhantomData<T>`.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
pub struct Uuid<T>([u8; 16], #[bitcode(skip)] PhantomData<fn() -> T>);

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

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Uuid(e) => write!(f, "uuid: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Uuid(e) => Some(e),
        }
    }
}

impl From<uuid::Error> for Error {
    fn from(err: uuid::Error) -> Self {
        Error::Uuid(err)
    }
}
