//! IPC message types. [`Request`] and [`Response`] are the wire
//! enums; [`Call`] plus one payload struct per variant drive
//! [`Handler::call`]; [`ResponseError`] covers the reply-inspection
//! failures [`Call::from_response`] can raise. The wire codec for
//! [`Request`] / [`Response`] lives in [`super::frame`].
//!
//! [`Handler::call`]: crate::protocol::Handler::call

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

/// Failure variants produced when extracting an expected [`Response`]
/// variant via the internal `Response::expect_*` methods. Converts
/// into [`crate::protocol::Error`] via `?` inside [`Call::from_response`]
/// implementations.
#[derive(Debug)]
pub enum ResponseError {
    /// The peer's server-side handler returned a failure; the string is
    /// the message it put inside [`Response::Error`].
    Remote(String),
    /// The peer returned a valid [`Response`] of the wrong variant.
    UnexpectedResponse,
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

/// One typed RPC call: a payload struct that knows how to become a
/// [`Request`] and how to extract its declared [`Call::Response`]
/// out of a matching [`Response`] variant.
///
/// Every [`Request`] variant has a mirror payload struct that impls
/// this trait, so [`crate::protocol::Handler::call`] can accept any
/// call uniformly: `handler.call(Unlock { .. }).await?`.
pub trait Call: Sized {
    /// The value returned when the matching [`Response`] variant
    /// comes back.
    type Response;
    /// Build the wire [`Request`] for this call.
    fn into_request(self) -> Request;
    /// Extract [`Self::Response`] from the peer's reply.
    fn from_response(resp: Response) -> Result<Self::Response, ResponseError>;
}

/// Generate a unit-struct [`Call`] impl for a [`Request`] variant
/// with no fields. Used by [`Status`], [`ListUsers`], [`LockAll`].
macro_rules! unit_call {
    ($name:ident, $variant:ident, $resp_ty:ty, $extractor:ident) => {
        #[doc = concat!("Payload for [`Request::", stringify!($variant), "`].")]
        pub struct $name;
        impl Call for $name {
            type Response = $resp_ty;
            fn into_request(self) -> Request {
                Request::$variant
            }
            fn from_response(r: Response) -> Result<Self::Response, ResponseError> {
                r.$extractor()
            }
        }
    };
}

unit_call!(Status, Status, Vec<String>, expect_users);
unit_call!(ListUsers, ListUsers, Vec<String>, expect_users);
unit_call!(LockAll, LockAll, (), expect_ok);

/// Payload for [`Request::Unlock`].
pub struct Unlock {
    pub username: String,
    pub password: Password,
}
impl Call for Unlock {
    type Response = ();
    fn into_request(self) -> Request {
        Request::Unlock {
            username: self.username,
            password: self.password,
        }
    }
    fn from_response(r: Response) -> Result<(), ResponseError> {
        r.expect_ok()
    }
}

/// Payload for [`Request::Lock`].
pub struct Lock {
    pub username: String,
}
impl Call for Lock {
    type Response = ();
    fn into_request(self) -> Request {
        Request::Lock {
            username: self.username,
        }
    }
    fn from_response(r: Response) -> Result<(), ResponseError> {
        r.expect_ok()
    }
}

/// Payload for [`Request::List`].
pub struct List {
    pub username: String,
    pub queries: Vec<String>,
}
impl Call for List {
    type Response = Vec<(Uuid<Record>, Label)>;
    fn into_request(self) -> Request {
        Request::List {
            username: self.username,
            queries: self.queries,
        }
    }
    fn from_response(r: Response) -> Result<Self::Response, ResponseError> {
        r.expect_index()
    }
}

/// Payload for [`Request::Fetch`].
pub struct Fetch {
    pub username: String,
    pub uuid: Uuid<Record>,
}
impl Call for Fetch {
    type Response = Record;
    fn into_request(self) -> Request {
        Request::Fetch {
            username: self.username,
            uuid: self.uuid,
        }
    }
    fn from_response(r: Response) -> Result<Record, ResponseError> {
        r.expect_record()
    }
}

/// Payload for [`Request::FindRecords`].
pub struct FindRecords {
    pub username: String,
    pub lot: String,
    pub query: String,
}
impl Call for FindRecords {
    type Response = Vec<(Uuid<Record>, Label)>;
    fn into_request(self) -> Request {
        Request::FindRecords {
            username: self.username,
            lot: self.lot,
            query: self.query,
        }
    }
    fn from_response(r: Response) -> Result<Self::Response, ResponseError> {
        r.expect_index()
    }
}

/// Payload for [`Request::GetRecord`].
pub struct GetRecord {
    pub username: String,
    pub lot: String,
    pub uuid: Uuid<Record>,
}
impl Call for GetRecord {
    type Response = Record;
    fn into_request(self) -> Request {
        Request::GetRecord {
            username: self.username,
            lot: self.lot,
            uuid: self.uuid,
        }
    }
    fn from_response(r: Response) -> Result<Record, ResponseError> {
        r.expect_record()
    }
}

/// Payload for [`Request::CreateRecord`].
pub struct CreateRecord {
    pub username: String,
    pub lot: String,
    pub label: Label,
    pub password: Password,
    pub extra: HashMap<String, String>,
}
impl Call for CreateRecord {
    type Response = Record;
    fn into_request(self) -> Request {
        Request::CreateRecord {
            username: self.username,
            lot: self.lot,
            label: self.label,
            password: self.password,
            extra: self.extra,
        }
    }
    fn from_response(r: Response) -> Result<Record, ResponseError> {
        r.expect_record()
    }
}

/// Payload for [`Request::GenerateRecord`].
pub struct GenerateRecord {
    pub username: String,
    pub lot: String,
    pub label: Label,
}
impl Call for GenerateRecord {
    type Response = Record;
    fn into_request(self) -> Request {
        Request::GenerateRecord {
            username: self.username,
            lot: self.lot,
            label: self.label,
        }
    }
    fn from_response(r: Response) -> Result<Record, ResponseError> {
        r.expect_record()
    }
}

/// Payload for [`Request::Register`].
pub struct Register {
    pub username: String,
    pub password: Password,
}
impl Call for Register {
    type Response = ();
    fn into_request(self) -> Request {
        Request::Register {
            username: self.username,
            password: self.password,
        }
    }
    fn from_response(r: Response) -> Result<(), ResponseError> {
        r.expect_ok()
    }
}

/// Payload for [`Request::Validate`].
pub struct Validate {
    pub username: String,
    pub password: Password,
}
impl Call for Validate {
    type Response = ();
    fn into_request(self) -> Request {
        Request::Validate {
            username: self.username,
            password: self.password,
        }
    }
    fn from_response(r: Response) -> Result<(), ResponseError> {
        r.expect_ok()
    }
}

/// Payload for [`Request::ListLots`].
pub struct ListLots {
    pub username: String,
}
impl Call for ListLots {
    type Response = Vec<(Uuid<Lot>, String)>;
    fn into_request(self) -> Request {
        Request::ListLots {
            username: self.username,
        }
    }
    fn from_response(r: Response) -> Result<Self::Response, ResponseError> {
        r.expect_lots()
    }
}

/// Payload for [`Request::CreateLot`].
pub struct CreateLot {
    pub username: String,
    pub lot: String,
}
impl Call for CreateLot {
    type Response = ();
    fn into_request(self) -> Request {
        Request::CreateLot {
            username: self.username,
            lot: self.lot,
        }
    }
    fn from_response(r: Response) -> Result<(), ResponseError> {
        r.expect_ok()
    }
}

/// Payload for [`Request::DeleteLot`].
pub struct DeleteLot {
    pub username: String,
    pub lot: String,
}
impl Call for DeleteLot {
    type Response = ();
    fn into_request(self) -> Request {
        Request::DeleteLot {
            username: self.username,
            lot: self.lot,
        }
    }
    fn from_response(r: Response) -> Result<(), ResponseError> {
        r.expect_ok()
    }
}

/// Payload for [`Request::History`].
pub struct History {
    pub username: String,
    pub lot: String,
    pub uuid: Uuid<Record>,
}
impl Call for History {
    type Response = Vec<RevisionEntry>;
    fn into_request(self) -> Request {
        Request::History {
            username: self.username,
            lot: self.lot,
            uuid: self.uuid,
        }
    }
    fn from_response(r: Response) -> Result<Self::Response, ResponseError> {
        r.expect_history()
    }
}

impl Response {
    /// Extract [`Response::Ok`]. Folds [`Response::Error`] and
    /// any other variant into [`ResponseError`].
    pub(crate) fn expect_ok(self) -> Result<(), ResponseError> {
        match self {
            Response::Ok => Ok(()),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::Users`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub(crate) fn expect_users(self) -> Result<Vec<String>, ResponseError> {
        match self {
            Response::Users(v) => Ok(v),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::Index`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub(crate) fn expect_index(self) -> Result<Vec<(Uuid<Record>, Label)>, ResponseError> {
        match self {
            Response::Index(v) => Ok(v),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::Record`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub(crate) fn expect_record(self) -> Result<Record, ResponseError> {
        match self {
            Response::Record(r) => Ok(r),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::Lots`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub(crate) fn expect_lots(self) -> Result<Vec<(Uuid<Lot>, String)>, ResponseError> {
        match self {
            Response::Lots(v) => Ok(v),
            Response::Error(msg) => Err(ResponseError::Remote(msg)),
            _ => Err(ResponseError::UnexpectedResponse),
        }
    }

    /// Extract [`Response::History`]. Folds [`Response::Error`] and any
    /// other variant into [`ResponseError`].
    pub(crate) fn expect_history(self) -> Result<Vec<RevisionEntry>, ResponseError> {
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

    fn sample_label() -> Label {
        Label::from(LabelName::Simple("github.com".into()))
            .add_extra("username", "alice")
            .unwrap()
    }

    fn sample_record() -> Record {
        use crate::Lot;
        use crate::record::Data;
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
    fn response_error_converts_into_protocol_error() {
        let e: crate::protocol::Error = ResponseError::Remote("x".into()).into();
        match e {
            crate::protocol::Error::Remote(m) => assert_eq!(m, "x"),
            other => panic!("expected Error::Remote, got {other:?}"),
        }
        let e: crate::protocol::Error = ResponseError::UnexpectedResponse.into();
        match e {
            crate::protocol::Error::Unexpected => {}
            other => panic!("expected Error::Unexpected, got {other:?}"),
        }
    }
}
