//! IPC message types plus the I/O that serves them.
//!
//! The wire format is a 4-byte big-endian length prefix followed by a
//! `bitcode`-encoded body. There is no separate framing module — framing is
//! how a [`Request`] or [`Response`] gets onto and off of a socket, so it
//! lives alongside the type definition.

use bitcode::{Decode, Encode};
use std::io::{self, Read, Write};
use valet::{Record, password::Password, record::Label, uuid::Uuid};

/// Maximum allowed frame payload, 16 MiB. The daemon never returns anywhere
/// near this much data; the cap exists to bound client-side allocations if
/// a peer misbehaves.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// A message sent from a client to the daemon. Each variant is answered by
/// exactly one [`Response`].
#[derive(Encode, Decode)]
pub enum Request {
    /// Derive the user's key from `password` and unlock the session. On
    /// success the daemon replies with [`Response::Session`].
    Unlock {
        username: String,
        password: Password,
    },
    /// Ask the daemon for records matching any of the given service
    /// identifiers (typically hostnames supplied by an autofill extension).
    /// An empty vector asks for every record in the active lot. The daemon
    /// replies with [`Response::Index`].
    List { queries: Vec<String> },
    /// Fetch the full decrypted [`Record`] by uuid, including password
    /// material. The daemon replies with [`Response::Record`].
    Fetch { uuid: Uuid<Record> },
}

/// A message sent from the daemon back to the client in reply to a
/// [`Request`]. Every request produces exactly one response; [`Error`] may
/// be returned in place of any success variant.
///
/// [`Error`]: Response::Error
#[derive(Encode, Decode)]
pub enum Response {
    /// Label-and-uuid pairs for every matching record, mirroring
    /// [`RecordIndex`](::valet::record::RecordIndex). No password material
    /// crosses the wire here; fetch by uuid when fill is requested.
    Index(Vec<(Uuid<Record>, Label)>),
    /// A single decrypted record returned in response to [`Request::Fetch`].
    Record(Record),
    /// Opaque session token returned after a successful [`Request::Unlock`].
    /// The client presents this on subsequent requests to prove it is
    /// attached to an unlocked session.
    Session { token: String },
    /// Human-readable error message. Returned in place of any success
    /// variant when the daemon cannot satisfy the request (locked session,
    /// unknown uuid, bad password, I/O failure, ...).
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
        record::{Data, Label, LabelName},
    };

    fn sample_label() -> Label {
        Label::from(LabelName::Simple("github.com".into()))
            .add_extra("username", "alice")
            .unwrap()
    }

    fn sample_record() -> Record {
        let lot = Lot::new("test-lot");
        Record::new(
            &lot,
            sample_label(),
            Data::new("hunter22".try_into().unwrap()),
        )
    }

    #[test]
    fn request_round_trip_list() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let sent = Request::List {
            queries: vec!["github.com".into(), "example.com".into()],
        };
        sent.write(&mut a).unwrap();
        let got = Request::read(&mut b).unwrap();
        match got {
            Request::List { queries } => assert_eq!(queries, vec!["github.com", "example.com"]),
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
    fn response_round_trip_index() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let rec = sample_record();
        let uuid = rec.uuid().clone();
        let label = rec.label().clone();
        let sent = Response::Index(vec![(uuid.clone(), label)]);
        sent.write(&mut a).unwrap();
        let got = Response::read(&mut b).unwrap();
        match got {
            Response::Index(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0.to_uuid(), uuid.to_uuid());
                assert_eq!(entries[0].1.name(), &LabelName::Simple("github.com".into()));
                assert_eq!(
                    entries[0].1.extra().get("username"),
                    Some(&"alice".to_string())
                );
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn response_round_trip_record() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        let rec = sample_record();
        let uuid = rec.uuid().clone();
        Response::Record(rec).write(&mut a).unwrap();
        let got = Response::read(&mut b).unwrap();
        match got {
            Response::Record(record) => {
                assert_eq!(record.uuid().to_uuid(), uuid.to_uuid());
                assert_eq!(record.password().to_string(), "hunter22");
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
