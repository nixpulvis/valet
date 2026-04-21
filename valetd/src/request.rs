//! IPC message types and their framing.
//!
//! Wire format: 4-byte big-endian length prefix followed by a `bitcode`-encoded
//! [`Request`] or [`Response`] body. The sync `write`/`read` helpers compile
//! everywhere; the `*_async` tokio variants are gated behind the `native`
//! feature so a wasm-friendly consumer (the browser extension) can depend
//! on the wire types without pulling in tokio.

use bitcode::{Decode, Encode};
use std::collections::HashMap;
use std::io::{self, Read, Write};
#[cfg(feature = "native")]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use valet::{
    Record,
    password::Password,
    record::{Label, LabelName},
    uuid::Uuid,
};

/// Maximum allowed frame payload, 16 MiB. The daemon never returns anywhere
/// near this much data; the cap exists to bound client-side allocations if
/// a peer misbehaves.
pub const MAX_FRAME_LEN: usize = 16 * 1024 * 1024;

/// A message sent from a client to the daemon. Each variant is answered by
/// exactly one [`Response`] (possibly [`Response::Error`]).
#[derive(Encode, Decode, Debug, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Request {
    /// List currently unlocked usernames. Answered with [`Response::Users`].
    Status,
    /// List every registered username. Answered with [`Response::Users`].
    ListUsers,
    /// Derive the user's key and cache the unlocked [`valet::user::User`] on the
    /// daemon. Answered with [`Response::Ok`] or [`Response::Error`].
    Unlock {
        username: String,
        password: Password,
    },
    /// Drop the cached keys for one user. Answered with [`Response::Ok`].
    Lock { username: String },
    /// Drop every cached user. Answered with [`Response::Ok`].
    LockAll,
    /// Cross-lot query-language search. `queries` are parsed as
    /// [`valet::record::Query`]; empty means every record in every lot this
    /// user has access to. Answered with [`Response::Index`].
    List {
        username: String,
        queries: Vec<String>,
    },
    /// Fetch one decrypted [`Record`] by uuid. The daemon searches the user's
    /// lots until it finds a match. Answered with [`Response::Record`].
    Fetch {
        username: String,
        uuid: Uuid<Record>,
    },
    // TODO: fold this into `List` by adding a `Query::Domain` variant to
    // `valet::record::Query` that carries the symmetric-suffix match
    // semantics. Then `FindRecords`, `domain_matches`, and
    // `label_matches_domain` can all go away.
    /// Per-lot domain match. `query` is a host string; the daemon returns the
    /// label-and-uuid pairs of every matching record, without decrypting
    /// passwords. Answered with [`Response::Index`].
    FindRecords {
        username: String,
        lot: String,
        query: String,
    },
    /// Fetch one decrypted [`Record`] by uuid in a specific lot. Answered
    /// with [`Response::Record`].
    GetRecord {
        username: String,
        lot: String,
        uuid: Uuid<Record>,
    },
    /// Create a new record with a caller-supplied password. Answered with
    /// [`Response::Record`] carrying the stored record.
    CreateRecord {
        username: String,
        lot: String,
        label: Label,
        password: Password,
        extra: HashMap<String, String>,
    },
    /// Create a new record with a daemon-generated password. Answered with
    /// [`Response::Record`] carrying the stored record.
    GenerateRecord {
        username: String,
        lot: String,
        label: Label,
    },
}

/// A message sent from the daemon back to the client in reply to a
/// [`Request`]. Every request produces exactly one response; [`Error`] may
/// be returned in place of any success variant.
///
/// [`Error`]: Response::Error
#[derive(Encode, Decode, Debug, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Response {
    /// Generic success with no payload (Unlock, Lock, LockAll).
    Ok,
    /// Username list (Status, ListUsers).
    Users(Vec<String>),
    /// Label-and-uuid pairs for every matching record. No password material
    /// crosses the wire. Answered by [`Request::List`] and
    /// [`Request::FindRecords`].
    Index(Vec<(Uuid<Record>, Label)>),
    /// A single decrypted record (Fetch, GetRecord, CreateRecord,
    /// GenerateRecord).
    Record(Record),
    /// Human-readable error message, returned in place of any success variant
    /// when the daemon cannot satisfy the request.
    // TODO: Make a proper Error enum for this too
    Error(String),
}

/// Return `true` if a domain label matches a query host, using the loose
/// suffix rule that powers the [`Request::FindRecords`] RPC. The match is
/// symmetric across the dot boundary: `"sub.example.com"` matches
/// `"example.com"` and vice-versa; `"example.com"` does not match
/// `"other.com"`.
pub fn domain_matches(record_domain: &str, query: &str) -> bool {
    let r = record_domain.to_lowercase();
    let q = query.to_lowercase();
    r == q || q.ends_with(&format!(".{r}")) || r.ends_with(&format!(".{q}"))
}

/// Return `true` if `label` is a domain label matching `query` under
/// [`domain_matches`].
pub fn label_matches_domain(label: &Label, query: &str) -> bool {
    match label.name() {
        LabelName::Domain { domain, .. } => domain_matches(domain, query),
        _ => false,
    }
}

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
#[cfg(feature = "native")]
pub async fn send_frame_async<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    check_len(payload.len())?;
    let len = u32::try_from(payload.len()).expect("checked above");
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(payload).await?;
    w.flush().await
}

/// Read one length-prefixed frame from `r`. Inverse of [`send_frame_async`].
#[cfg(feature = "native")]
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
    #[cfg(feature = "native")]
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
    #[cfg(feature = "native")]
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

/// RPC-layer error for valetd clients. `T` is the transport-specific
/// failure type: [`io::Error`] for the socket client and FFI, a richer
/// enum for the browser extension's native-messaging path.
///
/// [`Rpc`](Self::Rpc) means no application-level reply arrived
/// (transport failure, decode failure, disconnect).
/// [`Response`](Self::Response) carries a reply-inspection failure
/// ([`ResponseError`]): either a peer-reported error or an unexpected
/// response variant.
#[derive(Debug)]
pub enum Error<T> {
    /// The RPC round-trip itself failed.
    Rpc(T),
    /// A valid reply arrived but could not be interpreted as the
    /// expected [`Response`] variant.
    Response(ResponseError),
}

impl<T: std::fmt::Display> std::fmt::Display for Error<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Rpc(e) => write!(f, "rpc: {e}"),
            Error::Response(r) => write!(f, "{r}"),
        }
    }
}

impl<T: std::fmt::Debug + std::fmt::Display> std::error::Error for Error<T> {}

/// Failure variants produced when extracting an expected [`Response`]
/// variant via the `Response::expect_*` methods. Converts into
/// [`Error`] for any `T` via `?`.
#[derive(Debug)]
pub enum ResponseError {
    /// The peer's server-side handler returned a failure; the string is
    /// the message it put inside [`Response::Error`].
    Remote(String),
    /// The peer returned a valid [`Response`] of the wrong variant.
    UnexpectedResponse,
}

impl<T> From<ResponseError> for Error<T> {
    fn from(e: ResponseError) -> Self {
        Error::Response(e)
    }
}

impl std::fmt::Display for ResponseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResponseError::Remote(msg) => write!(f, "remote: {msg}"),
            ResponseError::UnexpectedResponse => write!(f, "unexpected response variant"),
        }
    }
}

impl std::error::Error for ResponseError {}

impl Response {
    /// Extract [`Response::Ok`]. Folds [`Response::Error`] and
    /// any other variant into [`ResponseError`].
    pub fn expect_ok(self) -> Result<(), ResponseError> {
        match self {
            Response::Ok => Ok(()),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::Users`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub fn expect_users(self) -> Result<Vec<String>, ResponseError> {
        match self {
            Response::Users(v) => Ok(v),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::Index`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub fn expect_index(self) -> Result<Vec<(Uuid<Record>, Label)>, ResponseError> {
        match self {
            Response::Index(v) => Ok(v),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::Record`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub fn expect_record(self) -> Result<Record, ResponseError> {
        match self {
            Response::Record(r) => Ok(r),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }
}

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

    #[test]
    fn domain_matches_symmetric() {
        assert!(domain_matches("example.com", "example.com"));
        assert!(domain_matches("example.com", "sub.example.com"));
        assert!(domain_matches("sub.example.com", "example.com"));
        assert!(!domain_matches("example.com", "other.com"));
    }

    #[test]
    fn expect_ok_variants() {
        Response::Ok.expect_ok().unwrap();
        match Response::Error("nope".into()).expect_ok() {
            Err(ResponseError::Remote(m)) => assert_eq!(m, "nope"),
            other => panic!("expected Remote, got {other:?}"),
        }
        match Response::Users(vec![]).expect_ok() {
            Err(ResponseError::UnexpectedResponse) => {}
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
    }

    #[test]
    fn expect_users_variants() {
        assert_eq!(
            Response::Users(vec!["alice".into()]).expect_users().unwrap(),
            vec!["alice".to_string()],
        );
        match Response::Error("locked".into()).expect_users() {
            Err(ResponseError::Remote(m)) => assert_eq!(m, "locked"),
            other => panic!("expected Remote, got {other:?}"),
        }
        match Response::Ok.expect_users() {
            Err(ResponseError::UnexpectedResponse) => {}
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
    }

    #[test]
    fn expect_index_variants() {
        let rec = sample_record();
        let uuid = rec.uuid().clone();
        let label = rec.label().clone();
        let got = Response::Index(vec![(uuid.clone(), label)])
            .expect_index()
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.to_uuid(), uuid.to_uuid());

        match Response::Error("boom".into()).expect_index() {
            Err(ResponseError::Remote(m)) => assert_eq!(m, "boom"),
            other => panic!("expected Remote, got {other:?}"),
        }
        match Response::Ok.expect_index() {
            Err(ResponseError::UnexpectedResponse) => {}
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
    }

    #[test]
    fn expect_record_variants() {
        let rec = sample_record();
        let uuid = rec.uuid().clone();
        let got = Response::Record(rec).expect_record().unwrap();
        assert_eq!(got.uuid().to_uuid(), uuid.to_uuid());

        match Response::Error("missing".into()).expect_record() {
            Err(ResponseError::Remote(m)) => assert_eq!(m, "missing"),
            other => panic!("expected Remote, got {other:?}"),
        }
        match Response::Users(vec![]).expect_record() {
            Err(ResponseError::UnexpectedResponse) => {}
            other => panic!("expected UnexpectedResponse, got {other:?}"),
        }
    }

    #[test]
    fn response_error_converts_into_error() {
        let e: Error<io::Error> = ResponseError::Remote("x".into()).into();
        match e {
            Error::Response(ResponseError::Remote(m)) => assert_eq!(m, "x"),
            other => panic!("expected Error::Response(Remote), got {other:?}"),
        }
        let e: Error<io::Error> = ResponseError::UnexpectedResponse.into();
        match e {
            Error::Response(ResponseError::UnexpectedResponse) => {}
            other => panic!("expected Error::Response(UnexpectedResponse), got {other:?}"),
        }
    }
}
