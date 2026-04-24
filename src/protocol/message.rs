//! IPC message types: [`Request`], [`Response`], the `expect_*` reply
//! helpers, and the [`Error`] / [`ResponseError`] shapes typed
//! handlers return. The wire codec for these types lives in
//! [`super::frame`].

use crate::{
    Lot, Record,
    password::Password,
    record::{Label, LabelName},
    uuid::Uuid,
};
use bitcode::{Decode, Encode};
use std::collections::HashMap;

/// A message sent from a client to the handler. Each variant is answered by
/// exactly one [`Response`] (possibly [`Response::Error`]).
#[derive(Encode, Decode, Debug, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Request {
    /// List currently unlocked usernames. Answered with [`Response::Users`].
    Status,
    /// List every registered username. Answered with [`Response::Users`].
    ListUsers,
    /// Derive the user's key and cache the unlocked [`crate::user::User`] on
    /// the handler. Answered with [`Response::Ok`] or [`Response::Error`].
    Unlock {
        username: String,
        password: Password,
    },
    /// Drop the cached keys for one user. Answered with [`Response::Ok`].
    Lock { username: String },
    /// Drop every cached user. Answered with [`Response::Ok`].
    LockAll,
    /// Cross-lot query-language search. `queries` are parsed as
    /// [`crate::record::Query`]; empty means every record in every lot this
    /// user has access to. Answered with [`Response::Index`].
    List {
        username: String,
        queries: Vec<String>,
    },
    /// Fetch one decrypted [`Record`] by uuid. The handler searches the
    /// user's lots until it finds a match. Answered with [`Response::Record`].
    Fetch {
        username: String,
        uuid: Uuid<Record>,
    },
    // TODO: fold this into `List` by adding a `Query::Domain` variant to
    // `valet::record::Query` that carries the symmetric-suffix match
    // semantics. Then `FindRecords`, `domain_matches`, and
    // `label_matches_domain` can all go away.
    /// Per-lot domain match. `query` is a host string; the handler returns
    /// the label-and-uuid pairs of every matching record, without
    /// decrypting passwords. Answered with [`Response::Index`].
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
    /// Create a new record with a handler-generated password. Answered with
    /// [`Response::Record`] carrying the stored record.
    GenerateRecord {
        username: String,
        lot: String,
        label: Label,
    },
    /// Register a new user and their default lot. Answered with
    /// [`Response::Ok`] or [`Response::Error`].
    Register {
        username: String,
        password: Password,
    },
    /// Verify a password against a user's stored salt + validation token
    /// without caching the unlocked state. Answered with [`Response::Ok`]
    /// or [`Response::Error`].
    Validate {
        username: String,
        password: Password,
    },
    /// List every lot the user has access to. Answered with
    /// [`Response::Lots`].
    ListLots { username: String },
    /// Create a new lot owned by the user. Answered with [`Response::Ok`].
    CreateLot { username: String, lot: String },
    /// Delete a lot. Answered with [`Response::Ok`].
    DeleteLot { username: String, lot: String },
    /// Walk the record's historical revisions (newest first). Answered
    /// with [`Response::History`].
    History {
        username: String,
        lot: String,
        uuid: Uuid<Record>,
    },
}

/// One historical revision of a record, as carried in
/// [`Response::History`]. A wire-friendly subset of the in-process
/// [`crate::record::Revision`] (no `storgit::CommitId`, no `SystemTime`
/// baggage).
#[derive(Encode, Decode, Debug)]
pub struct RevisionEntry {
    /// Commit timestamp, milliseconds since the Unix epoch. Negative
    /// for pre-1970 commits (clock skew).
    pub time_millis: i64,
    pub label: Label,
    pub password: Password,
}

/// A message sent from the handler back to the client in reply to a
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
    /// Lot uuid plus name list (ListLots). Sorted by name.
    Lots(Vec<(Uuid<Lot>, String)>),
    /// Record-revision list, newest first (History).
    History(Vec<RevisionEntry>),
    /// Human-readable error message, returned in place of any success variant
    /// when the handler cannot satisfy the request.
    // TODO: Make a proper Error enum for this too
    Error(String),
}

// TODO: goes away with the `Request::FindRecords` fold (see the TODO on
// that variant above).
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

// TODO: goes away with the `Request::FindRecords` fold (see the TODO on
// that variant above).
/// Return `true` if `label` is a domain label matching `query` under
/// [`domain_matches`].
pub fn label_matches_domain(label: &Label, query: &str) -> bool {
    match label.name() {
        LabelName::Domain { domain, .. } => domain_matches(domain, query),
        _ => false,
    }
}

/// RPC-layer error for valet handler clients. `T` is the transport-specific
/// failure type: [`std::io::Error`] for the socket handler and FFI, a richer
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

    /// Extract [`Response::Lots`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub fn expect_lots(self) -> Result<Vec<(Uuid<Lot>, String)>, ResponseError> {
        match self {
            Response::Lots(v) => Ok(v),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::History`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub fn expect_history(self) -> Result<Vec<RevisionEntry>, ResponseError> {
        match self {
            Response::History(v) => Ok(v),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Record;
    use crate::record::{Label, LabelName};
    use std::io;

    fn sample_label() -> Label {
        Label::from(LabelName::Simple("github.com".into()))
            .add_extra("username", "alice")
            .unwrap()
    }

    fn sample_record() -> Record {
        use crate::record::Data;
        use crate::Lot;
        let lot = Lot::new("test-lot");
        Record::new(
            &lot,
            sample_label(),
            Data::new("hunter22".try_into().unwrap()),
        )
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
            Response::Users(vec!["alice".into()])
                .expect_users()
                .unwrap(),
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
