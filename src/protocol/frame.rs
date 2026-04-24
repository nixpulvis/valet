//! Wire framing for [`Request`] and [`Response`].
//!
//! Both forms share the same bitcode body; they differ only in the
//! outer envelope each transport expects:
//!
//! * Length-prefixed (4-byte big-endian length, then payload). The
//!   sync [`Frame::send`]/[`Frame::recv`] compile everywhere; the
//!   [`Frame::send_async`]/[`Frame::recv_async`] tokio variants are
//!   gated behind the private `_async-io` feature so a
//!   wasm-friendly consumer (the browser extension) can depend on
//!   [`Request`]/[`Response`] without pulling in tokio.
//! * Base64 of the bitcode body, for stuffing inside the browser
//!   native-messaging JSON envelope.
//!
//! [`Request`]: super::message::Request
//! [`Response`]: super::message::Response

use super::message::{Request, Response};
use bitcode::{Decode, Encode};
use std::io::{self, Read, Write};
#[cfg(feature = "_async-io")]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum allowed frame payload, 16 MiB. The daemon never returns anywhere
/// near this much data; the cap exists to bound client-side allocations if
/// a peer misbehaves.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

fn check_len(len: usize) -> io::Result<()> {
    if len > MAX_FRAME_LEN {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "frame exceeds MAX_FRAME_LEN",
        ))
    } else {
        Ok(())
    }
}

fn write_frame<W: Write>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    check_len(payload.len())?;
    let len = u32::try_from(payload.len()).expect("checked above");
    w.write_all(&len.to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame<R: Read>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes)?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    check_len(len)?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// Write one length-prefixed frame of already-encoded bytes to `w`. The
/// payload is anything up to [`MAX_FRAME_LEN`] bytes; callers that want
/// the typed encode-then-send should use [`Frame::send_async`] instead.
#[cfg(feature = "_async-io")]
pub async fn send_frame_async<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    check_len(payload.len())?;
    let len = u32::try_from(payload.len()).expect("checked above");
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(payload).await?;
    w.flush().await
}

/// Read one length-prefixed frame from `r`. Inverse of [`send_frame_async`].
#[cfg(feature = "_async-io")]
pub async fn recv_frame_async<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    r.read_exact(&mut len_bytes).await?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    check_len(len)?;
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

fn decode_err<E: std::fmt::Display>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("bitcode decode: {e}"))
}

/// Shared framing for wire messages. The length-prefixed `send`/`recv`
/// helpers speak the Unix-socket wire format used between the daemon and
/// its remote clients. The `encode_base64`/`decode_base64` helpers speak
/// the envelope the browser native-messaging shim stuffs inside its JSON
/// frames; same bitcode payload, different outer wrapper.
///
/// TODO: before a proper release, add a version discriminator (one magic
/// byte or a leading u16) to both the length-prefixed and base64 forms.
/// Bitcode enum tags are positional, so adding or removing a `Request` /
/// `Response` variant silently misdecodes across mismatched peers today.
pub trait Frame: Encode + for<'de> Decode<'de> + Sized {
    /// Bitcode-encode `self` into a freshly-allocated buffer, without any
    /// framing. The length-prefix and base64 helpers below are built on
    /// top of this; callers that already have their own framing (the
    /// browser native-messaging shim's embedded mode) use it directly.
    fn encode(&self) -> Vec<u8> {
        bitcode::encode(self)
    }

    /// Inverse of [`encode`](Self::encode). Decode failures are surfaced
    /// as `io::Error` with kind [`io::ErrorKind::InvalidData`].
    fn decode(bytes: &[u8]) -> io::Result<Self> {
        bitcode::decode(bytes).map_err(decode_err)
    }

    /// Bitcode-encode `self` and write it as one length-prefixed frame.
    fn send<W: Write>(&self, w: &mut W) -> io::Result<()> {
        write_frame(w, &self.encode())
    }

    /// Read one length-prefixed frame and bitcode-decode it.
    fn recv<R: Read>(r: &mut R) -> io::Result<Self> {
        Self::decode(&read_frame(r)?)
    }

    /// Async [`send`](Self::send) over a tokio writer.
    #[cfg(feature = "_async-io")]
    fn send_async<W: AsyncWrite + Unpin + Send>(
        &self,
        w: &mut W,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send
    where
        Self: Sync,
    {
        async move { send_frame_async(w, &self.encode()).await }
    }

    /// Async [`recv`](Self::recv) over a tokio reader.
    #[cfg(feature = "_async-io")]
    fn recv_async<R: AsyncRead + Unpin + Send>(
        r: &mut R,
    ) -> impl std::future::Future<Output = io::Result<Self>> + Send {
        async move { Self::decode(&recv_frame_async(r).await?) }
    }

    /// Bitcode-encode `self` and base64 it, for embedding in the browser
    /// native-messaging JSON envelope.
    fn encode_base64(&self) -> String {
        use base64::{Engine, engine::general_purpose::STANDARD};
        STANDARD.encode(self.encode())
    }

    /// Inverse of [`encode_base64`](Self::encode_base64).
    fn decode_base64(b64: &str) -> Result<Self, DecodeError> {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let bytes = STANDARD.decode(b64).map_err(DecodeError::Base64)?;
        bitcode::decode(&bytes).map_err(DecodeError::Bitcode)
    }
}

impl Frame for Request {}
impl Frame for Response {}

/// Errors from [`Frame::decode_base64`].
#[derive(Debug)]
pub enum DecodeError {
    /// The base64 envelope was malformed.
    Base64(base64::DecodeError),
    /// The bitcode payload could not be decoded.
    Bitcode(bitcode::Error),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Base64(e) => write!(f, "base64: {e}"),
            DecodeError::Bitcode(e) => write!(f, "bitcode: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::{Data, Label, LabelName};
    use crate::{Lot, Record};
    use std::os::unix::net::UnixStream;

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
            username: "alice".into(),
            queries: vec!["github.com".into(), "example.com".into()],
        };
        sent.send(&mut a).unwrap();
        let got = Request::recv(&mut b).unwrap();
        match got {
            Request::List { username, queries } => {
                assert_eq!(username, "alice");
                assert_eq!(queries, vec!["github.com", "example.com"]);
            }
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
        sent.send(&mut a).unwrap();
        let got = Request::recv(&mut b).unwrap();
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
        sent.send(&mut a).unwrap();
        let got = Response::recv(&mut b).unwrap();
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
        Response::Record(rec).send(&mut a).unwrap();
        let got = Response::recv(&mut b).unwrap();
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
        Response::Error("locked".into()).send(&mut a).unwrap();
        let got = Response::recv(&mut b).unwrap();
        match got {
            Response::Error(msg) => assert_eq!(msg, "locked"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn oversize_frame_rejected() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        a.write_all(&(MAX_FRAME_LEN as u32 + 1).to_be_bytes())
            .unwrap();
        match Response::recv(&mut b) {
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData),
            Ok(_) => panic!("expected InvalidData error"),
        }
    }
}
