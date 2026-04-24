//! Integration tests for `Client<Embedded>` against an in-memory
//! SQLite DB. Exercises the real `LocalHandler`-backed dispatch that
//! lives inside the embedded client, including the unlock cache and
//! the failed-unlock delay.

use crate::common::embedded_client_with_user;
use valet::Handler;

#[tokio::test(flavor = "multi_thread")]
async fn register_unlock_status() {
    let client = embedded_client_with_user("alice", "sesame").await;
    assert_eq!(client.status().await.unwrap(), vec!["alice".to_string()]);
    assert_eq!(
        client.list_users().await.unwrap(),
        vec!["alice".to_string()]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn register_leaves_user_unlocked() {
    use valet::Client;
    use valet::db::Database;
    use valet::protocol::embedded::Embedded;

    let db = Database::new("sqlite://:memory:").await.unwrap();
    let client = Client::<Embedded>::new(db);
    client
        .register("bob".into(), "hunter22".try_into().unwrap())
        .await
        .unwrap();
    // No explicit unlock: Register is supposed to leave the user cached.
    assert_eq!(client.status().await.unwrap(), vec!["bob".to_string()]);
    let lots = client.list_lots("bob".into()).await.unwrap();
    let names: Vec<&str> = lots.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(names, vec!["main"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn lock_drops_cached_user() {
    let client = embedded_client_with_user("alice", "sesame").await;
    client.lock("alice".into()).await.unwrap();
    assert!(client.status().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn lock_all_drops_everyone() {
    let client = embedded_client_with_user("alice", "sesame").await;
    client.lock_all().await.unwrap();
    assert!(client.status().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn unlock_wrong_password_errors() {
    let client = embedded_client_with_user("alice", "sesame").await;
    // A further unlock attempt with a bad password should error and
    // not touch the existing unlocked cache.
    let err = client
        .unlock("alice".into(), "wrong".try_into().unwrap())
        .await
        .unwrap_err();
    match err {
        valet::protocol::Error::Remote(_) => {}
        other => panic!("unexpected: {other:?}"),
    }
    assert_eq!(client.status().await.unwrap(), vec!["alice".to_string()]);
}

#[tokio::test(flavor = "multi_thread")]
async fn create_and_fetch_record() {
    let client = embedded_client_with_user("alice", "sesame").await;
    let created = client
        .create_record(
            "alice".into(),
            valet::lot::DEFAULT_LOT.into(),
            "example.com".parse().unwrap(),
            "hunter2".try_into().unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    let fetched = client
        .fetch("alice".into(), created.uuid().clone())
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
            .create_record(
                "alice".into(),
                valet::lot::DEFAULT_LOT.into(),
                host.parse().unwrap(),
                "pw".try_into().unwrap(),
                Default::default(),
            )
            .await
            .unwrap();
    }
    let entries = client.list("alice".into(), vec![]).await.unwrap();
    assert_eq!(entries.len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn find_records_domain_suffix() {
    let client = embedded_client_with_user("alice", "sesame").await;
    client
        .create_record(
            "alice".into(),
            valet::lot::DEFAULT_LOT.into(),
            "alice@github.com".parse().unwrap(),
            "pw".try_into().unwrap(),
            Default::default(),
        )
        .await
        .unwrap();
    let entries = client
        .find_records(
            "alice".into(),
            valet::lot::DEFAULT_LOT.into(),
            "gist.github.com".into(),
        )
        .await
        .unwrap();
    assert_eq!(entries.len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn generate_record_produces_password() {
    let client = embedded_client_with_user("alice", "sesame").await;
    let record = client
        .generate_record(
            "alice".into(),
            valet::lot::DEFAULT_LOT.into(),
            "gen.example".parse().unwrap(),
        )
        .await
        .unwrap();
    assert!(!record.password().as_bytes().is_empty());
}
