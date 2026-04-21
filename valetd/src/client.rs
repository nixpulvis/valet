//! Synchronous Unix-socket client. Used by the macOS AutoFill extension
//! through the FFI and by tests.
//!
//! The client is stateless: every record-touching request names its target
//! `username` (and, where relevant, `lot`) explicitly. Session state lives
//! in `valetd`. It tracks which users are unlocked and which lot keys are
//! cached, scoped by `(username, lot)` and cleared by the idle reaper or
//! explicit [`Client::lock`] / [`Client::lock_all`] calls. Callers
//! typically pick a username via [`Client::status`] and thread it into
//! subsequent [`Client::list`] / [`Client::fetch`] calls.

use crate::request::{Frame, Request, Response};
use std::collections::HashMap;
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
        req.send(&mut self.stream)?;
        Ok(Response::recv(&mut self.stream)?)
    }

    fn expect_ok(&mut self, req: Request) -> Result<(), Error> {
        match self.round_trip(req)? {
            Response::Ok => Ok(()),
            Response::Error(msg) => Err(Error::Remote(msg)),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    fn expect_users(&mut self, req: Request) -> Result<Vec<String>, Error> {
        match self.round_trip(req)? {
            Response::Users(list) => Ok(list),
            Response::Error(msg) => Err(Error::Remote(msg)),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    fn expect_record(&mut self, req: Request) -> Result<Record, Error> {
        match self.round_trip(req)? {
            Response::Record(r) => Ok(r),
            Response::Error(msg) => Err(Error::Remote(msg)),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    fn expect_index(&mut self, req: Request) -> Result<Vec<(Uuid<Record>, Label)>, Error> {
        match self.round_trip(req)? {
            Response::Index(entries) => Ok(entries),
            Response::Error(msg) => Err(Error::Remote(msg)),
            _ => Err(Error::UnexpectedResponse),
        }
    }

    pub fn status(&mut self) -> Result<Vec<String>, Error> {
        self.expect_users(Request::Status)
    }

    pub fn list_users(&mut self) -> Result<Vec<String>, Error> {
        self.expect_users(Request::ListUsers)
    }

    pub fn unlock(&mut self, username: &str, password: Password) -> Result<(), Error> {
        self.expect_ok(Request::Unlock {
            username: username.to_owned(),
            password,
        })
    }

    pub fn lock(&mut self, username: &str) -> Result<(), Error> {
        self.expect_ok(Request::Lock {
            username: username.to_owned(),
        })
    }

    pub fn lock_all(&mut self) -> Result<(), Error> {
        self.expect_ok(Request::LockAll)
    }

    pub fn list(
        &mut self,
        username: &str,
        queries: &[String],
    ) -> Result<Vec<(Uuid<Record>, Label)>, Error> {
        self.expect_index(Request::List {
            username: username.to_owned(),
            queries: queries.to_vec(),
        })
    }

    pub fn fetch(&mut self, username: &str, uuid: &Uuid<Record>) -> Result<Record, Error> {
        self.expect_record(Request::Fetch {
            username: username.to_owned(),
            uuid: uuid.clone(),
        })
    }

    pub fn find_records(
        &mut self,
        username: &str,
        lot: &str,
        query: &str,
    ) -> Result<Vec<(Uuid<Record>, Label)>, Error> {
        self.expect_index(Request::FindRecords {
            username: username.to_owned(),
            lot: lot.to_owned(),
            query: query.to_owned(),
        })
    }

    pub fn get_record(
        &mut self,
        username: &str,
        lot: &str,
        uuid: &Uuid<Record>,
    ) -> Result<Record, Error> {
        self.expect_record(Request::GetRecord {
            username: username.to_owned(),
            lot: lot.to_owned(),
            uuid: uuid.clone(),
        })
    }

    pub fn create_record(
        &mut self,
        username: &str,
        lot: &str,
        label: Label,
        password: Password,
        extra: HashMap<String, String>,
    ) -> Result<Record, Error> {
        self.expect_record(Request::CreateRecord {
            username: username.to_owned(),
            lot: lot.to_owned(),
            label,
            password,
            extra,
        })
    }

    pub fn generate_record(
        &mut self,
        username: &str,
        lot: &str,
        label: Label,
    ) -> Result<Record, Error> {
        self.expect_record(Request::GenerateRecord {
            username: username.to_owned(),
            lot: lot.to_owned(),
            label,
        })
    }
}
