//! Integration tests for the [`Handler`] trait's default method
//! surface, exercised through `StubHandler` (a no-DB, no-socket fake).
//!
//! These are the reference: if a typed method's dispatch or
//! `expect_*` glue is wrong, a stub test catches it without pulling
//! SQLite or sockets into the fault surface.
//!
//! [`Handler`]: valet::Handler

// Only the stub fixture is needed here; the other helpers in
// `tests/common/` (embedded / tempdir / envelope) would look like
// dead code in this binary. Cherry-picking just `common/stub.rs` via
// `#[path]` avoids pulling them in.
#[path = "common/stub.rs"]
mod stub;

use stub::{EXAMPLE_UUID, STUB_LOT, STUB_USER, StubHandler, YCOMBINATOR_UUID};
use valet::{Handler, Record, uuid::Uuid};

#[tokio::test(flavor = "multi_thread")]
async fn status_reflects_unlock_and_lock() {
    let stub = StubHandler::new();
    assert!(stub.status().await.unwrap().is_empty());
    stub.unlock(STUB_USER.into(), "pw".try_into().unwrap())
        .await
        .unwrap();
    assert_eq!(stub.status().await.unwrap(), vec![STUB_USER]);
    stub.lock(STUB_USER.into()).await.unwrap();
    assert!(stub.status().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn list_users_returns_fixture() {
    let stub = StubHandler::new();
    assert_eq!(stub.list_users().await.unwrap(), vec![STUB_USER]);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_empty_query_returns_all() {
    let stub = StubHandler::new();
    let entries = stub.list(STUB_USER.into(), vec![]).await.unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_literal_query_filters() {
    let stub = StubHandler::new();
    let entries = stub
        .list(STUB_USER.into(), vec!["stub::alice@ycombinator.com".into()])
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn list_invalid_query_surfaces_error() {
    let stub = StubHandler::new();
    let err = stub
        .list(STUB_USER.into(), vec!["foo<k=v".into()])
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
    let record = stub.fetch(STUB_USER.into(), uuid.clone()).await.unwrap();
    assert_eq!(record.uuid().to_uuid(), uuid.to_uuid());
    assert_eq!(record.password().to_string(), "hunter22");
}

#[tokio::test(flavor = "multi_thread")]
async fn fetch_missing_errors() {
    let stub = StubHandler::new();
    let bogus: Uuid<Record> = Uuid::parse("00000000-0000-0000-0000-000000000000").unwrap();
    let err = stub.fetch(STUB_USER.into(), bogus).await.unwrap_err();
    match err {
        valet::protocol::Error::Remote(_) => {}
        other => panic!("unexpected: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn find_records_domain_suffix_match() {
    let stub = StubHandler::new();
    let entries = stub
        .find_records(
            STUB_USER.into(),
            STUB_LOT.into(),
            "news.ycombinator.com".into(),
        )
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
        .find_records(STUB_USER.into(), "other".into(), "example.com".into())
        .await
        .unwrap();
    assert!(entries.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn get_record_by_uuid() {
    let stub = StubHandler::new();
    let uuid: Uuid<Record> = Uuid::parse(EXAMPLE_UUID).unwrap();
    let record = stub
        .get_record(STUB_USER.into(), STUB_LOT.into(), uuid.clone())
        .await
        .unwrap();
    assert_eq!(record.uuid().to_uuid(), uuid.to_uuid());
}

#[tokio::test(flavor = "multi_thread")]
async fn create_record_is_rejected() {
    let stub = StubHandler::new();
    let err = stub
        .create_record(
            STUB_USER.into(),
            STUB_LOT.into(),
            "foo".parse().unwrap(),
            "pw".try_into().unwrap(),
            Default::default(),
        )
        .await
        .unwrap_err();
    match err {
        valet::protocol::Error::Remote(_) => {}
        other => panic!("unexpected: {other:?}"),
    }
}
