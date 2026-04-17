//! Synchronous Unix-socket client. The future macOS extension drives this
//! from a background `DispatchQueue`; other Rust clients (CLI helper, tests)
//! use it directly.

use crate::request::{Request, Response};
use std::{io, os::unix::net::UnixStream, path::Path};
use valet::{Record, password::Password, record::Label, uuid::Uuid};

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Remote(String),
    UnexpectedResponse,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io error: {e}"),
            Error::Remote(msg) => write!(f, "remote error: {msg}"),
            Error::UnexpectedResponse => write!(f, "unexpected response variant"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

pub struct Client {
    stream: UnixStream,
}

impl Client {
    pub fn connect(path: &Path) -> io::Result<Self> {
        Ok(Client {
            stream: UnixStream::connect(path)?,
        })
    }

    fn round_trip(&mut self, req: Request) -> Result<Response, Error> {
        req.write(&mut self.stream)?;
        Ok(Response::read(&mut self.stream)?)
    }

    pub fn unlock(&mut self, username: &str, password: Password) -> Result<String, Error> {
        match self.round_trip(Request::Unlock {
            username: username.to_owned(),
            password,
        })? {
            Response::Session { token } => Ok(token),
            Response::Error(msg) => Err(Error::Remote(msg)),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub fn list(&mut self, queries: &[String]) -> Result<Vec<(Uuid<Record>, Label)>, Error> {
        match self.round_trip(Request::List {
            queries: queries.to_vec(),
        })? {
            Response::Index(entries) => Ok(entries),
            Response::Error(msg) => Err(Error::Remote(msg)),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub fn fetch(&mut self, uuid: &Uuid<Record>) -> Result<Record, Error> {
        match self.round_trip(Request::Fetch { uuid: uuid.clone() })? {
            Response::Record(record) => Ok(record),
            Response::Error(msg) => Err(Error::Remote(msg)),
            _ => Err(Error::UnexpectedResponse),
        }
    }
}
