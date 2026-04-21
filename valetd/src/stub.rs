//! In-process fake server. Answers requests from a fixed set of records
//! without going through a socket or touching a database. Used by the macOS
//! extension when built with `--features stub`, and by FFI tests.
//!
//! The stub is a [`Handler`] just like [`DaemonHandler`]: transports call
//! `stub.handle(req).await`. Record creation requests are rejected; idle
//! timeouts don't exist; unlocking accepts any password. Otherwise the
//! response shape matches what the daemon would produce.
//!
//! [`Handler`]: crate::server::Handler
//! [`DaemonHandler`]: crate::server::DaemonHandler

use crate::request::{Request, Response, label_matches_domain};
use crate::server::Handler;
use std::io;
use tokio::sync::Mutex;
use valet::{
    Lot, Record,
    record::{Data, Label, LabelName, Query},
    uuid::Uuid,
};

const LOT_NAME: &str = "stub";
const STUB_USER: &str = "stub-user";
// Fixed UUIDs so the macOS App and Extension processes, which each
// instantiate their own Stub, agree on record identity. The App writes
// these uuids into ASCredentialIdentityStore; the Extension's fetch path
// resolves an autofill request back to the same record by looking them up
// here. Randomizing per process would break autofill.
const YCOMBINATOR_UUID: &str = "01900000-0000-7000-8000-00000000a1c0";
const EXAMPLE_UUID: &str = "01900000-0000-7000-8000-00000000e8a3";

/// In-process request handler backed by a fixed in-memory record set.
pub struct Stub {
    state: Mutex<StubState>,
}

struct StubState {
    lot_name: String,
    records: Vec<Record>,
    active_user: Option<String>,
}

impl Stub {
    pub fn new() -> Self {
        let lot = Lot::new(LOT_NAME);
        let records = vec![
            Record::with_uuid(
                Uuid::parse(YCOMBINATOR_UUID).unwrap(),
                &lot,
                Label::from(LabelName::Simple("ycombinator.com".into()))
                    .add_extra("username", "alice")
                    .unwrap()
                    .add_extra("url", "https://news.ycombinator.com")
                    .unwrap(),
                Data::new("hunter22".try_into().unwrap()),
            ),
            Record::with_uuid(
                Uuid::parse(EXAMPLE_UUID).unwrap(),
                &lot,
                Label::from(LabelName::Simple("example.com".into()))
                    .add_extra("username", "bob")
                    .unwrap()
                    .add_extra("url", "https://example.com")
                    .unwrap(),
                Data::new("correct horse battery".try_into().unwrap()),
            ),
        ];
        Stub {
            state: Mutex::new(StubState {
                lot_name: lot.name().to_string(),
                records,
                active_user: None,
            }),
        }
    }
}

impl Default for Stub {
    fn default() -> Self {
        Self::new()
    }
}

impl Handler for Stub {
    async fn handle(&self, req: Request) -> io::Result<Response> {
        let mut st = self.state.lock().await;
        Ok(dispatch(&mut st, req))
    }
}

fn dispatch(st: &mut StubState, req: Request) -> Response {
    match req {
        Request::Status => Response::Users(st.active_user.iter().cloned().collect()),
        Request::ListUsers => Response::Users(vec![STUB_USER.to_owned()]),
        Request::Unlock { username, .. } => {
            st.active_user = Some(username);
            Response::Ok
        }
        Request::Lock { username } => {
            if st.active_user.as_deref() == Some(username.as_str()) {
                st.active_user = None;
            }
            Response::Ok
        }
        Request::LockAll => {
            st.active_user = None;
            Response::Ok
        }
        Request::List { queries, .. } => {
            let parsed = match queries
                .iter()
                .map(|s| s.parse::<Query>())
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(p) => p,
                Err(e) => return Response::Error(format!("invalid query: {e}")),
            };
            let entries = st
                .records
                .iter()
                .filter(|r| {
                    parsed.is_empty()
                        || parsed
                            .iter()
                            .any(|q| q.matches_lot(&st.lot_name) && q.matches_label(r.label()))
                })
                .map(|r| (r.uuid().clone(), r.label().clone()))
                .collect();
            Response::Index(entries)
        }
        Request::Fetch { uuid, .. } | Request::GetRecord { uuid, .. } => {
            let needle = uuid.to_uuid();
            for r in &st.records {
                if r.uuid().to_uuid() == needle {
                    return Response::Record(clone_record(r));
                }
            }
            Response::Error(format!("no record with uuid {uuid}"))
        }
        Request::FindRecords { lot, query, .. } => {
            if lot != st.lot_name {
                return Response::Index(Vec::new());
            }
            let entries = st
                .records
                .iter()
                .filter(|r| label_matches_domain(r.label(), &query))
                .map(|r| (r.uuid().clone(), r.label().clone()))
                .collect();
            Response::Index(entries)
        }
        Request::CreateRecord { .. } => Response::Error("stub: create_record not supported".into()),
        Request::GenerateRecord { .. } => {
            Response::Error("stub: generate_record not supported".into())
        }
    }
}

// Record doesn't implement Clone, but it does derive bitcode Encode/Decode,
// so round-tripping through a buffer gives us a deep copy.
fn clone_record(r: &Record) -> Record {
    bitcode::decode(&bitcode::encode(r)).expect("record round-trip")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn handle(stub: &Stub, req: Request) -> Response {
        stub.handle(req).await.expect("stub never returns io err")
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
    }

    #[test]
    fn list_empty_queries_returns_all() {
        rt().block_on(async {
            let stub = Stub::new();
            match handle(
                &stub,
                Request::List {
                    username: STUB_USER.into(),
                    queries: vec![],
                },
            )
            .await
            {
                Response::Index(e) => assert_eq!(e.len(), 2),
                other => panic!("unexpected {other:?}"),
            }
        });
    }

    #[test]
    fn list_filters_by_literal_query() {
        rt().block_on(async {
            let stub = Stub::new();
            match handle(
                &stub,
                Request::List {
                    username: STUB_USER.into(),
                    queries: vec!["stub::ycombinator.com".into()],
                },
            )
            .await
            {
                Response::Index(hits) => {
                    assert_eq!(hits.len(), 1);
                    assert_eq!(hits[0].1.name(), &LabelName::Simple("ycombinator.com".into()));
                }
                other => panic!("unexpected {other:?}"),
            }
        });
    }

    #[test]
    fn list_filters_by_regex_query() {
        rt().block_on(async {
            let stub = Stub::new();
            match handle(
                &stub,
                Request::List {
                    username: STUB_USER.into(),
                    queries: vec!["stub::~.*".into()],
                },
            )
            .await
            {
                Response::Index(hits) => assert_eq!(hits.len(), 2),
                other => panic!("unexpected {other:?}"),
            }
        });
    }

    #[test]
    fn list_non_matching_query_returns_empty() {
        rt().block_on(async {
            let stub = Stub::new();
            match handle(
                &stub,
                Request::List {
                    username: STUB_USER.into(),
                    queries: vec!["stub::nope".into()],
                },
            )
            .await
            {
                Response::Index(hits) => assert!(hits.is_empty()),
                other => panic!("unexpected {other:?}"),
            }
        });
    }

    #[test]
    fn list_invalid_query_errors() {
        rt().block_on(async {
            let stub = Stub::new();
            match handle(
                &stub,
                Request::List {
                    username: STUB_USER.into(),
                    queries: vec!["foo<k=v".into()],
                },
            )
            .await
            {
                Response::Error(msg) => assert!(msg.contains("invalid query")),
                other => panic!("unexpected {other:?}"),
            }
        });
    }

    #[test]
    fn fetch_returns_matching_record() {
        rt().block_on(async {
            let stub = Stub::new();
            let all = match handle(
                &stub,
                Request::List {
                    username: STUB_USER.into(),
                    queries: vec![],
                },
            )
            .await
            {
                Response::Index(e) => e,
                other => panic!("unexpected {other:?}"),
            };
            let uuid = all[0].0.clone();
            match handle(
                &stub,
                Request::Fetch {
                    username: STUB_USER.into(),
                    uuid: uuid.clone(),
                },
            )
            .await
            {
                Response::Record(r) => assert_eq!(r.uuid().to_uuid(), uuid.to_uuid()),
                other => panic!("unexpected {other:?}"),
            }
        });
    }

    #[test]
    fn fetch_missing_returns_error() {
        rt().block_on(async {
            let stub = Stub::new();
            let bogus: Uuid<Record> =
                Uuid::parse("00000000-0000-0000-0000-000000000000").unwrap();
            match handle(
                &stub,
                Request::Fetch {
                    username: STUB_USER.into(),
                    uuid: bogus,
                },
            )
            .await
            {
                Response::Error(_) => {}
                other => panic!("unexpected {other:?}"),
            }
        });
    }
}
