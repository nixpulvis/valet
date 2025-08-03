use std::marker::PhantomData;
use std::ops::Deref;

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
