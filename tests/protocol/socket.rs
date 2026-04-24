//! Integration tests for `Client<Socket>` <-> `Server<Socket>`.
//!
//! Every test here uses `StubHandler` as the `Server<Socket>`'s
//! backing provider; nothing in this file reaches for a real DB or
//! another protocol. That keeps the gate on this submodule at
//! `protocol-socket` alone and isolates wire-level regressions from
//! storage regressions. Cross-protocol composition lives in
//! [`super::multi`].

use crate::common::stub::{EXAMPLE_UUID, STUB_LOT, STUB_USER, StubHandler};
use crate::common::tempdir;
use std::sync::Arc;
use std::time::Duration;
use valet::protocol::socket::Socket;
use valet::{Client, Handler, Record, Server, uuid::Uuid};

/// Happy path: a `Client<Socket>` exercises `status` + `fetch`
/// against a spawned `Server<Socket>` backed by a `StubHandler`.
#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_covers_status_and_fetch() {
    let tmp = tempdir();
    let sock_path = tmp.join("valet.sock");

    let stub = Arc::new(StubHandler::new());
    // Unlock a user on the stub so `status` returns something
    // non-empty on the other side of the wire.
    stub.unlock(STUB_USER.into(), "pw".try_into().unwrap())
        .await
        .unwrap();

    let server = Server::<Socket>::bind(&sock_path).await.expect("bind");
    let server_task = tokio::spawn(async move {
        let _ = server.serve(stub).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let socket_client = Client::<Socket>::connect(&sock_path)
        .await
        .expect("connect");
    assert_eq!(
        socket_client.status().await.unwrap(),
        vec![STUB_USER.to_string()]
    );
    let uuid: Uuid<Record> = Uuid::parse(EXAMPLE_UUID).unwrap();
    let fetched = socket_client
        .fetch(STUB_USER.into(), uuid.clone())
        .await
        .unwrap();
    assert_eq!(fetched.uuid().to_uuid(), uuid.to_uuid());
    assert_eq!(fetched.password().to_string(), "correct horse battery");

    drop(socket_client);
    server_task.abort();
}

/// Errors from the handler travel the wire and surface on the client
/// as `Error::Remote(msg)`, not transport failures.
#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_propagates_handler_errors() {
    let tmp = tempdir();
    let sock_path = tmp.join("valet.sock");

    let stub = Arc::new(StubHandler::new());
    let server = Server::<Socket>::bind(&sock_path).await.expect("bind");
    let server_task = tokio::spawn(async move {
        let _ = server.serve(stub).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let socket_client = Client::<Socket>::connect(&sock_path)
        .await
        .expect("connect");
    let err = socket_client
        .create_record(
            STUB_USER.into(),
            STUB_LOT.into(),
            "alice@example.com".parse().unwrap(),
            "pw".try_into().unwrap(),
            Default::default(),
        )
        .await
        .expect_err("stub should reject create_record");
    match err {
        valet::protocol::Error::Remote(msg) => assert!(msg.contains("create_record")),
        other => panic!("expected Remote, got {other:?}"),
    }

    drop(socket_client);
    server_task.abort();
}

/// Two concurrent `Client<Socket>`s talking to one `Server<Socket>`
/// each get their own connection and both see correct replies; the
/// per-connection mutex keeps their frame streams aligned.
#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_handles_concurrent_clients() {
    let tmp = tempdir();
    let sock_path = tmp.join("valet.sock");

    let stub = Arc::new(StubHandler::new());
    stub.unlock(STUB_USER.into(), "pw".try_into().unwrap())
        .await
        .unwrap();
    let server = Server::<Socket>::bind(&sock_path).await.expect("bind");
    let server_task = tokio::spawn({
        let handler = stub.clone();
        async move {
            let _ = server.serve(handler).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let a = Client::<Socket>::connect(&sock_path)
        .await
        .expect("connect a");
    let b = Client::<Socket>::connect(&sock_path)
        .await
        .expect("connect b");

    let (ra, rb) = tokio::join!(a.status(), b.list_users());
    assert_eq!(ra.unwrap(), vec![STUB_USER.to_string()]);
    assert_eq!(rb.unwrap(), vec![STUB_USER.to_string()]);

    let (ra, rb) = tokio::join!(a.list_users(), b.status());
    assert_eq!(ra.unwrap(), vec![STUB_USER.to_string()]);
    assert_eq!(rb.unwrap(), vec![STUB_USER.to_string()]);

    drop(a);
    drop(b);
    server_task.abort();
}

/// When a `Client<Socket>` disconnects mid-session the server's
/// per-connection task returns cleanly on EOF; the accept loop keeps
/// serving. Spawning a second client proves the listener survived.
#[tokio::test(flavor = "multi_thread")]
async fn roundtrip_clean_eof_does_not_break_listener() {
    let tmp = tempdir();
    let sock_path = tmp.join("valet.sock");

    let stub = Arc::new(StubHandler::new());
    stub.unlock(STUB_USER.into(), "pw".try_into().unwrap())
        .await
        .unwrap();
    let server = Server::<Socket>::bind(&sock_path).await.expect("bind");
    let server_task = tokio::spawn(async move {
        let _ = server.serve(stub).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    {
        let a = Client::<Socket>::connect(&sock_path)
            .await
            .expect("connect a");
        assert_eq!(a.status().await.unwrap(), vec![STUB_USER.to_string()]);
    }

    let b = Client::<Socket>::connect(&sock_path)
        .await
        .expect("connect b");
    assert_eq!(b.status().await.unwrap(), vec![STUB_USER.to_string()]);

    drop(b);
    server_task.abort();
}
