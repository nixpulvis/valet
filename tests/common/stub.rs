//! In-memory fake [`SendHandler`] for integration tests.
//!
//! Answers from a fixed set of records: never touches SQLite, never
//! opens a socket. Record-mutating requests are rejected; unlocking
//! accepts any password. Used to drive the typed call surface
//! without a DB.
//!
//! [`SendHandler`]: valet::SendHandler

use std::io;
use tokio::sync::Mutex;
use valet::{
    Lot, Record,
    protocol::{
        SendHandler,
        message::{Request, Response, label_matches_domain},
    },
    record::{Data, Label, LabelName, Query},
    uuid::Uuid,
};

pub const STUB_USER: &str = "stub-user";
pub const STUB_LOT: &str = "stub";
pub const STUB_LOT_UUID: &str = "01900000-0000-7000-8000-000000001007";
pub const YCOMBINATOR_UUID: &str = "01900000-0000-7000-8000-00000000a1c0";
pub const EXAMPLE_UUID: &str = "01900000-0000-7000-8000-00000000e8a3";

pub struct StubHandler {
    state: Mutex<StubState>,
}

struct StubState {
    records: Vec<Record>,
    active_user: Option<String>,
}

impl StubHandler {
    pub fn new() -> Self {
        let lot = Lot::new(STUB_LOT);
        let records = vec![
            Record::with_uuid(
                Uuid::parse(YCOMBINATOR_UUID).unwrap(),
                &lot,
                Label::from(LabelName::Domain {
                    id: "alice".into(),
                    domain: "ycombinator.com".into(),
                })
                .add_extra("url", "https://news.ycombinator.com")
                .unwrap(),
                Data::new("hunter22".try_into().unwrap()),
            ),
            Record::with_uuid(
                Uuid::parse(EXAMPLE_UUID).unwrap(),
                &lot,
                Label::from(LabelName::Domain {
                    id: "bob".into(),
                    domain: "example.com".into(),
                })
                .add_extra("url", "https://example.com")
                .unwrap(),
                Data::new("correct horse battery".try_into().unwrap()),
            ),
        ];
        StubHandler {
            state: Mutex::new(StubState {
                records,
                active_user: None,
            }),
        }
    }
}

impl Default for StubHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SendHandler for StubHandler {
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
                            .any(|q| q.matches_lot(STUB_LOT) && q.matches_label(r.label()))
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
            if lot != STUB_LOT {
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
        Request::Register { .. } => Response::Error("stub: register not supported".into()),
        Request::Validate { .. } => Response::Ok,
        Request::ListLots { .. } => Response::Lots(vec![(
            Uuid::parse(STUB_LOT_UUID).unwrap(),
            STUB_LOT.to_owned(),
        )]),
        Request::CreateLot { .. } => Response::Error("stub: create_lot not supported".into()),
        Request::DeleteLot { .. } => Response::Error("stub: delete_lot not supported".into()),
        Request::History { .. } => Response::Error("stub: history not supported".into()),
    }
}

// `Record` doesn't implement `Clone`, but it does derive bitcode
// `Encode`/`Decode`, so round-tripping through a buffer gives a deep
// copy.
fn clone_record(r: &Record) -> Record {
    bitcode::decode(&bitcode::encode(r)).expect("record round-trip")
}
