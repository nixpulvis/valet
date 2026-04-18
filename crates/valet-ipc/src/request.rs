//! IPC message types plus the I/O that serves them.
//!
//! The wire format is a 4-byte big-endian length prefix followed by a
//! `bitcode`-encoded body. There is no separate framing module — framing is
//! how a [`Request`] or [`Response`] gets onto and off of a socket, so it
//! lives alongside the type definition.

use bitcode::{Decode, Encode};
use std::io::{self, Read, Write};
use valet::{Record, password::Password, uuid::Uuid};

/// Maximum allowed frame payload, 16 MiB. The daemon never returns anywhere
/// near this much data; the cap exists to bound client-side allocations if
/// a peer misbehaves.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

#[derive(Encode, Decode)]
pub enum Request {
    Unlock {
        username: String,
        password: Password,
    },
    List {
        service_identifiers: Vec<String>,
    },
    Fetch {
        uuid: Uuid<Record>,
    },
}

#[derive(Encode, Decode)]
pub enum Response {
    Records(Vec<Record>),
    Record(Record),
    Session { token: String },
    Error(String),
}

fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload exceeds MAX_FRAME_LEN",
        ));
    }
    let len = u32::try_from(payload.len()).expect("checked above");
    w.write_all(&len.to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds MAX_FRAME_LEN",
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn decode_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bitcode decode: {e}"))
}

impl Request {
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        write_frame(w, &bitcode::encode(self))
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let buf = read_frame(r)?;
        bitcode::decode(&buf).map_err(decode_err)
    }
}

impl Response {
    pub fn write<W: Write>(&self, w: &mut W) -> io::Result<()> {
        write_frame(w, &bitcode::encode(self))
    }

    pub fn read<R: Read>(r: &mut R) -> io::Result<Self> {
        let buf = read_frame(r)?;
        bitcode::decode(&buf).map_err(decode_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    use valet::{
        Lot, Record,
        record::{Data, Label},
    };

    fn sample_record() -> Record {
        let lot = Lot::new("test-lot");
        Record::new(
            &lot,
            Data::new(
                Label::Simple("github.com".into()),
                "hunter22".try_into().unwrap(),
            )
            .add_extra("username".into(), "alice".into()),
        )
    }

    #[test]
    fn request_round_trip_list() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let sent = Request::List {
            service_identifiers: vec!["github.com".into(), "example.com".into()],
        };
        sent.write(&mut a).unwrap();
        let got = Request::read(&mut b).unwrap();
        match got {
            Request::List {
                service_identifiers,
            } => assert_eq!(service_identifiers, vec!["github.com", "example.com"]),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn request_round_trip_unlock() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let sent = Request::Unlock {
            username: "alice".into(),
            password: "s3cret!!".try_into().unwrap(),
        };
        sent.write(&mut a).unwrap();
        let got = Request::read(&mut b).unwrap();
        match got {
            Request::Unlock { username, password } => {
                assert_eq!(username, "alice");
                assert_eq!(password.to_string(), "s3cret!!");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_round_trip_records() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let rec = sample_record();
        let sent = Response::Records(vec![rec]);
        sent.write(&mut a).unwrap();
        let got = Response::read(&mut b).unwrap();
        match got {
            Response::Records(records) => {
                assert_eq!(records.len(), 1);
                assert_eq!(records[0].label(), &Label::Simple("github.com".into()));
                assert_eq!(records[0].password().to_string(), "hunter22");
                assert_eq!(
                    records[0].data().extra().get("username"),
                    Some(&"alice".to_string())
                );
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_round_trip_error() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        Response::Error("locked".into()).write(&mut a).unwrap();
        let got = Response::read(&mut b).unwrap();
        match got {
            Response::Error(msg) => assert_eq!(msg, "locked"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn oversize_frame_rejected() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        // Write a bogus length header that exceeds MAX_FRAME_LEN.
        a.write_all(&(MAX_FRAME_LEN as u32 + 1).to_be_bytes())
            .unwrap();
        match Response::read(&mut b) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            Ok(_) => panic!("expected InvalidData error"),
        }
    }
}
