//! Integration tests for the [`SendHandler`] trait's call surface,
//! exercised through `StubHandler` (a no-DB, no-socket fake).
//!
//! These are the reference: if typed call dispatch or `expect_*` glue
//! is wrong, a stub test catches it without pulling SQLite or sockets
//! into the fault surface.
//!
//! [`SendHandler`]: valet::SendHandler

// Only the stub fixture is needed here; the other helpers in
// `tests/common/` (embedded / tempdir / envelope) would look like
// dead code in this binary. Cherry-picking just `common/stub.rs` via
// `#[path]` avoids pulling them in.
#[path = "common/stub.rs"]
mod stub;

use stub::{EXAMPLE_UUID, STUB_LOT, STUB_USER, StubHandler, YCOMBINATOR_UUID};
use valet::protocol::message::{
    CreateRecord, Fetch, FindRecords, GetRecord, List, ListUsers, Lock, Status, Unlock,
};
use valet::{Record, SendHandler, uuid::Uuid};

#[tokio::test(flavor = "multi_thread")]
async fn status_reflects_unlock_and_lock() {
    let stub = StubHandler::new();
    assert!(stub.call(Status).await.unwrap().is_empty());
    stub.call(Unlock {
        username: STUB_USER.into(),
        password: "pw".try_into().unwrap(),
    })
    .await
    .unwrap();
    assert_eq!(stub.call(Status).await.unwrap(), vec![STUB_USER]);
    stub.call(Lock {
        username: STUB_USER.into(),
    })
    .await
    .unwrap();
    assert!(stub.call(Status).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn list_users_returns_fixture() {
    let stub = StubHandler::new();
    assert_eq!(stub.call(ListUsers).await.unwrap(), vec![STUB_USER]);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_empty_query_returns_all() {
    let stub = StubHandler::new();
    let entries = stub
        .call(List {
            username: STUB_USER.into(),
            queries: vec![],
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_literal_query_filters() {
    let stub = StubHandler::new();
    let entries = stub
        .call(List {
            username: STUB_USER.into(),
            queries: vec!["stub::alice@ycombinator.com".into()],
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_invalid_query_surfaces_error() {
    let stub = StubHandler::new();
    let err = stub
        .call(List {
            username: STUB_USER.into(),
            queries: vec!["foo<k=v".into()],
        })
        .await
        .unwrap_err();
    match err {
        valet::protocol::Error::Remote(msg) => assert!(msg.contains("invalid query")),
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_returns_record() {
    let stub = StubHandler::new();
    let uuid: Uuid<Record> = Uuid::parse(YCOMBINATOR_UUID).unwrap();
    let record = stub
        .call(Fetch {
            username: STUB_USER.into(),
            uuid: uuid.clone(),
        })
        .await
        .unwrap();
    assert_eq!(record.uuid().to_uuid(), uuid.to_uuid());
    assert_eq!(record.password().to_string(), "hunter22");
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_missing_errors() {
    let stub = StubHandler::new();
    let bogus: Uuid<Record> = Uuid::parse("00000000-0000-0000-0000-000000000000").unwrap();
    let err = stub
        .call(Fetch {
            username: STUB_USER.into(),
            uuid: bogus,
        })
        .await
        .unwrap_err();
    match err {
        valet::protocol::Error::Remote(_) => {}
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn find_records_domain_suffix_match() {
    let stub = StubHandler::new();
    let entries = stub
        .call(FindRecords {
            username: STUB_USER.into(),
            lot: STUB_LOT.into(),
            query: "news.ycombinator.com".into(),
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
    let uuid: Uuid<Record> = Uuid::parse(YCOMBINATOR_UUID).unwrap();
    assert_eq!(entries[0].0.to_uuid(), uuid.to_uuid());
}

#[tokio::test(flavor = "multi_thread")]
async fn find_records_unknown_lot_is_empty() {
    let stub = StubHandler::new();
    let entries = stub
        .call(FindRecords {
            username: STUB_USER.into(),
            lot: "other".into(),
            query: "example.com".into(),
        })
        .await
        .unwrap();
    assert!(entries.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn get_record_by_uuid() {
    let stub = StubHandler::new();
    let uuid: Uuid<Record> = Uuid::parse(EXAMPLE_UUID).unwrap();
    let record = stub
        .call(GetRecord {
            username: STUB_USER.into(),
            lot: STUB_LOT.into(),
            uuid: uuid.clone(),
        })
        .await
        .unwrap();
    assert_eq!(record.uuid().to_uuid(), uuid.to_uuid());
}

#[tokio::test(flavor = "multi_thread")]
async fn create_record_is_rejected() {
    let stub = StubHandler::new();
    let err = stub
        .call(CreateRecord {
            username: STUB_USER.into(),
            lot: STUB_LOT.into(),
            label: "foo".parse().unwrap(),
            password: "pw".try_into().unwrap(),
            extra: Default::default(),
        })
        .await
        .unwrap_err();
    match err {
        valet::protocol::Error::Remote(_) => {}
        other => panic!("unexpected: {other:?}"),
    }
}
