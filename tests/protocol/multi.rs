//! Cross-protocol composition tests.
//!
//! Every test in this file wires together more than one protocol. In
//! contrast to the per-protocol files (`embedded.rs`, `socket.rs`,
//! `native_msg.rs`), which each exercise one protocol in isolation
//! against either a stub or an in-memory DB, the tests here are
//! explicitly about how the protocols compose.

use crate::common::{embedded_client_with_user, envelope, tempdir};
use std::sync::Arc;
use std::time::Duration;
use valet::SendHandler;
use valet::protocol::message::{CreateRecord, Request, Response};
use valet::protocol::{Serve, SocketClient, SocketServer};

/// The non-embedded native-host mode, end-to-end. A native-messaging
/// server wraps a [`SocketClient`] that connects to a separate
/// [`SocketServer`] serving an [`EmbeddedHandler`]. Driving a request
/// through the native-messaging front end should produce the same
/// reply as driving it through the embedded handler directly.
///
/// [`SocketClient`]: valet::protocol::SocketClient
/// [`SocketServer`]: valet::protocol::SocketServer
/// [`EmbeddedHandler`]: valet::protocol::EmbeddedHandler
#[tokio::test(flavor = "multi_thread")]
async fn native_msg_relay_through_socket_server_to_embedded() {
    let tmp = tempdir();
    let sock_path = tmp.join("valet.sock");

    // Upstream: SocketServer + EmbeddedHandler.
    let embedded = Arc::new(embedded_client_with_user("alice", "sesame").await);
    let created = embedded
        .call(CreateRecord {
            username: "alice".into(),
            lot: valet::lot::DEFAULT_LOT.into(),
            label: "relay.example".parse().unwrap(),
            password: "hunter2".try_into().unwrap(),
            extra: Default::default(),
        })
        .await
        .unwrap();
    let upstream = SocketServer::bind(&sock_path).await.expect("bind");
    let upstream_task = tokio::spawn({
        let handler = embedded.clone();
        async move {
            let _ = upstream.serve(handler).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Downstream: NativeMessageServer whose handler is the
    // socket-client forwarding to upstream.
    let relay_client = Arc::new(
        SocketClient::connect(&sock_path)
            .await
            .expect("connect relay"),
    );
    let (mut to_server, mut from_server, nm_task) = envelope::spawn_server("socket", relay_client);

    envelope::send(
        &mut to_server,
        1,
        &Request::Fetch {
            username: "alice".into(),
            uuid: created.uuid().clone(),
        },
    )
    .await;
    let reply = envelope::recv(&mut from_server).await;
    assert_eq!(reply.backend, "socket");
    match envelope::payload_response(&reply) {
        Response::Record(r) => {
            assert_eq!(r.uuid().to_uuid(), created.uuid().to_uuid());
            assert_eq!(r.password().to_string(), "hunter2");
        }
        other => panic!("unexpected {other:?}"),
    }

    drop(to_server);
    let _ = nm_task.await;
    upstream_task.abort();
}
