//! Integration tests for [`EmbeddedHandler`] against an in-memory
//! SQLite DB. Exercises the real dispatch that lives inside the
//! embedded handler, including the unlock cache and the
//! failed-unlock delay.
//!
//! [`EmbeddedHandler`]: valet::protocol::EmbeddedHandler

use crate::common::embedded_client_with_user;
use valet::SendHandler;
use valet::protocol::message::{
    CreateRecord, Fetch, FindRecords, GenerateRecord, List, ListLots, ListUsers, Lock, LockAll,
    Register, Status, Unlock,
};

#[tokio::test(flavor = "multi_thread")]
async fn register_unlock_status() {
    let client = embedded_client_with_user("alice", "sesame").await;
    assert_eq!(
        client.call(Status).await.unwrap(),
        vec!["alice".to_string()]
    );
    assert_eq!(
        client.call(ListUsers).await.unwrap(),
        vec!["alice".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn register_leaves_user_unlocked() {
    use valet::db::Database;
    use valet::protocol::EmbeddedHandler;

    let db = Database::new("sqlite://:memory:").await.unwrap();
    let client = EmbeddedHandler::new(db, &tokio::runtime::Handle::current());
    client
        .call(Register {
            username: "bob".into(),
            password: "hunter22".try_into().unwrap(),
        })
        .await
        .unwrap();
    // No explicit unlock: Register is supposed to leave the user cached.
    assert_eq!(client.call(Status).await.unwrap(), vec!["bob".to_string()]);
    let lots = client
        .call(ListLots {
            username: "bob".into(),
        })
        .await
        .unwrap();
    let names: Vec<&str> = lots.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, vec!["main"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn lock_drops_cached_user() {
    let client = embedded_client_with_user("alice", "sesame").await;
    client
        .call(Lock {
            username: "alice".into(),
        })
        .await
        .unwrap();
    assert!(client.call(Status).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn lock_all_drops_everyone() {
    let client = embedded_client_with_user("alice", "sesame").await;
    client.call(LockAll).await.unwrap();
    assert!(client.call(Status).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn unlock_wrong_password_errors() {
    let client = embedded_client_with_user("alice", "sesame").await;
    // A further unlock attempt with a bad password should error and
    // not touch the existing unlocked cache.
    let err = client
        .call(Unlock {
            username: "alice".into(),
            password: "wrong".try_into().unwrap(),
        })
        .await
        .unwrap_err();
    match err {
        valet::protocol::Error::Remote(_) => {}
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(
        client.call(Status).await.unwrap(),
        vec!["alice".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn create_and_fetch_record() {
    let client = embedded_client_with_user("alice", "sesame").await;
    let created = client
        .call(CreateRecord {
            username: "alice".into(),
            lot: valet::lot::DEFAULT_LOT.into(),
            label: "example.com".parse().unwrap(),
            password: "hunter2".try_into().unwrap(),
            extra: Default::default(),
        })
        .await
        .unwrap();
    let fetched = client
        .call(Fetch {
            username: "alice".into(),
            uuid: created.uuid().clone(),
        })
        .await
        .unwrap();
    assert_eq!(fetched.uuid().to_uuid(), created.uuid().to_uuid());
    assert_eq!(fetched.password().to_string(), "hunter2");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_returns_created_records() {
    let client = embedded_client_with_user("alice", "sesame").await;
    for host in ["a.com", "b.com"] {
        client
            .call(CreateRecord {
                username: "alice".into(),
                lot: valet::lot::DEFAULT_LOT.into(),
                label: host.parse().unwrap(),
                password: "pw".try_into().unwrap(),
                extra: Default::default(),
            })
            .await
            .unwrap();
    }
    let entries = client
        .call(List {
            username: "alice".into(),
            queries: vec![],
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_records_domain_suffix() {
    let client = embedded_client_with_user("alice", "sesame").await;
    client
        .call(CreateRecord {
            username: "alice".into(),
            lot: valet::lot::DEFAULT_LOT.into(),
            label: "alice@github.com".parse().unwrap(),
            password: "pw".try_into().unwrap(),
            extra: Default::default(),
        })
        .await
        .unwrap();
    let entries = client
        .call(FindRecords {
            username: "alice".into(),
            lot: valet::lot::DEFAULT_LOT.into(),
            query: "gist.github.com".into(),
        })
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn generate_record_produces_password() {
    let client = embedded_client_with_user("alice", "sesame").await;
    let record = client
        .call(GenerateRecord {
            username: "alice".into(),
            lot: valet::lot::DEFAULT_LOT.into(),
            label: "gen.example".parse().unwrap(),
        })
        .await
        .unwrap();
    assert!(!record.password().as_bytes().is_empty());
}
