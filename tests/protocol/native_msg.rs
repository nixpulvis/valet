//! Integration tests for `Server<NativeMessage>` at the envelope
//! layer.
//!
//! `Server::<NativeMessage>::serve` reads stdin/stdout, which a test
//! can't substitute. [`valet::protocol::native_msg::serve_io`] runs
//! the same loop over any async byte streams; every test here drives
//! it through a pair of `tokio::io::duplex` pipes standing in for
//! the browser pipe (see [`crate::common::envelope`]).
//!
//! Every test uses `StubHandler` so this file is independent of the
//! other protocols. Cross-protocol composition (e.g. the
//! `Server<NativeMessage>` + `Client<Socket>` relay) lives in
//! [`super::multi`].

use crate::common::envelope;
use crate::common::stub::{STUB_LOT, STUB_USER, StubHandler};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use valet::protocol::native_msg::MAX_SIZE;
use valet::protocol::message::{Request, Response};

/// Round-trip a `Request::Status` through the native-messaging server
/// and decode the inner `Response::Users` from the reply payload.
#[tokio::test(flavor = "multi_thread")]
async fn envelope_roundtrip_status() {
    let (mut to_server, mut from_server, server) =
        envelope::spawn_server("embedded", Arc::new(StubHandler::new()));

    envelope::send(&mut to_server, 1, &Request::Status).await;
    let reply = envelope::recv(&mut from_server).await;
    assert_eq!(reply.backend, "embedded");
    let payload = reply.payload.as_ref().unwrap();
    assert_eq!(payload.id, 1);
    match envelope::payload_response(&reply) {
        Response::Users(u) => assert!(u.is_empty()),
        other => panic!("unexpected {other:?}"),
    }

    drop(to_server);
    let _ = server.await;
}

/// The `backend` tag the server was built with is echoed into every
/// reply, so the browser side can distinguish embedded vs socket
/// backends without a parallel channel.
#[tokio::test(flavor = "multi_thread")]
async fn echoes_backend_tag() {
    let (mut to_server, mut from_server, server) =
        envelope::spawn_server("socket", Arc::new(StubHandler::new()));
    envelope::send(&mut to_server, 7, &Request::ListUsers).await;
    let reply = envelope::recv(&mut from_server).await;
    assert_eq!(reply.backend, "socket");
    drop(to_server);
    let _ = server.await;
}

/// The caller's request id is preserved on the reply so the browser
/// can correlate out-of-order replies back to their pending callers.
#[tokio::test(flavor = "multi_thread")]
async fn preserves_request_id() {
    let (mut to_server, mut from_server, server) =
        envelope::spawn_server("embedded", Arc::new(StubHandler::new()));
    for id in [1u64, 42, 99, 1234] {
        envelope::send(&mut to_server, id, &Request::Status).await;
        let reply = envelope::recv(&mut from_server).await;
        assert_eq!(reply.payload.as_ref().unwrap().id, id);
    }
    drop(to_server);
    let _ = server.await;
}

/// A handler-level `Response::Error` is still a successful reply at
/// the envelope layer; the error payload lives inside the bitcode
/// `Response` on the `Ok(NativePayload)` branch.
#[tokio::test(flavor = "multi_thread")]
async fn handler_error_rides_inside_ok_payload() {
    let (mut to_server, mut from_server, server) =
        envelope::spawn_server("embedded", Arc::new(StubHandler::new()));
    // The stub rejects CreateRecord with a `Response::Error`.
    let req = Request::CreateRecord {
        username: STUB_USER.into(),
        lot: STUB_LOT.into(),
        label: "alice@example.com".parse().unwrap(),
        password: "pw".try_into().unwrap(),
        extra: Default::default(),
    };
    envelope::send(&mut to_server, 1, &req).await;
    let reply = envelope::recv(&mut from_server).await;
    match envelope::payload_response(&reply) {
        Response::Error(msg) => assert!(msg.contains("create_record")),
        other => panic!("expected Response::Error, got {other:?}"),
    }
    drop(to_server);
    let _ = server.await;
}

/// Malformed JSON inside an otherwise well-framed envelope: the server
/// keeps the connection alive and replies with an error envelope. The
/// next well-formed request still succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn malformed_json_replies_error_then_continues() {
    let (mut to_server, mut from_server, server) =
        envelope::spawn_server("embedded", Arc::new(StubHandler::new()));

    envelope::write_frame(&mut to_server, b"not json at all").await;
    let reply = envelope::recv(&mut from_server).await;
    match &reply.payload {
        Err(msg) => assert!(msg.contains("invalid json")),
        Ok(_) => panic!("expected Err payload"),
    }

    envelope::send(&mut to_server, 2, &Request::Status).await;
    let reply = envelope::recv(&mut from_server).await;
    assert!(matches!(
        envelope::payload_response(&reply),
        Response::Users(_)
    ));

    drop(to_server);
    let _ = server.await;
}

/// A well-formed envelope whose inner `request` field isn't valid
/// base64 surfaces as a payload error, not a server crash.
#[tokio::test(flavor = "multi_thread")]
async fn invalid_base64_replies_error() {
    let (mut to_server, mut from_server, server) =
        envelope::spawn_server("embedded", Arc::new(StubHandler::new()));
    let body = br#"{"id":3,"request":"not valid base64!!!"}"#;
    envelope::write_frame(&mut to_server, body).await;
    let reply = envelope::recv(&mut from_server).await;
    match &reply.payload {
        Err(msg) => assert!(msg.contains("invalid base64")),
        Ok(_) => panic!("expected Err payload"),
    }
    drop(to_server);
    let _ = server.await;
}

/// A declared frame length over `MAX_SIZE` terminates the loop
/// with `InvalidData`. The server doesn't try to allocate 1+ GiB.
#[tokio::test(flavor = "multi_thread")]
async fn oversize_frame_length_terminates() {
    let (mut to_server, _from_server, server) =
        envelope::spawn_server("embedded", Arc::new(StubHandler::new()));

    let bogus_len = (MAX_SIZE as u32 + 1).to_le_bytes();
    to_server.write_all(&bogus_len).await.unwrap();
    to_server.flush().await.unwrap();

    let result = server.await.unwrap();
    match result {
        Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidData),
        Ok(()) => panic!("expected InvalidData error"),
    }
}

/// Closing the read side cleanly returns `Ok(())` from the loop.
#[tokio::test(flavor = "multi_thread")]
async fn clean_eof_returns_ok() {
    let (to_server, _from_server, server) =
        envelope::spawn_server("embedded", Arc::new(StubHandler::new()));
    drop(to_server); // immediate EOF
    let result = server.await.unwrap();
    assert!(result.is_ok(), "expected clean EOF, got {result:?}");
}
