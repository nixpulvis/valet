//! Shared helpers for the integration tests.
//!
//! Each item is gated on exactly the protocol features whose test
//! submodules consume it, so no binary pulls in a helper that would
//! then look like dead code. The matrix:
//!
//! | item                      | needed when                                     |
//! |---------------------------|-------------------------------------------------|
//! | [`stub`]                  | `protocol-socket` or `protocol-native-msg-server`      |
//! | [`envelope`]              | `protocol-native-msg-server`                           |
//! | [`embedded_client_with_user`] | `protocol-embedded`                         |
//! | [`tempdir`]               | `protocol-socket` (socket + multi)              |

#[cfg(any(feature = "protocol-socket", feature = "protocol-native-msg-server"))]
pub mod stub;

#[cfg(feature = "protocol-native-msg-server")]
pub mod envelope;

/// Test-scoped wrapper around [`EmbeddedHandler`] that keeps the
/// [`tempfile::TempDir`] backing its data directory alive for as
/// long as the handler. Derefs to the inner handler so existing
/// `client.call(...)` sites are unchanged.
///
/// [`EmbeddedHandler`]: valet::protocol::EmbeddedHandler
#[cfg(feature = "protocol-embedded")]
pub struct EmbeddedTestHandler {
    handler: valet::protocol::EmbeddedHandler,
    _dir: tempfile::TempDir,
}

#[cfg(feature = "protocol-embedded")]
impl std::ops::Deref for EmbeddedTestHandler {
    type Target = valet::protocol::EmbeddedHandler;
    fn deref(&self) -> &Self::Target {
        &self.handler
    }
}

// Forward `SendHandler` so callers like `Serve::serve(handler)` that
// take `H: SendHandler` accept the wrapper directly. `Deref` would
// be enough for method calls, but trait bounds don't auto-deref.
#[cfg(feature = "protocol-embedded")]
impl valet::SendHandler for EmbeddedTestHandler {
    async fn handle(
        &self,
        req: valet::protocol::message::Request,
    ) -> std::io::Result<valet::protocol::message::Response> {
        self.handler.handle(req).await
    }
}

/// Register a user, create the default lot, unlock, and return the
/// resulting handler wrapped in [`EmbeddedTestHandler`]. Every
/// embedded / socket / native-messaging test starts from this state.
#[cfg(feature = "protocol-embedded")]
pub async fn embedded_client_with_user(username: &str, password: &str) -> EmbeddedTestHandler {
    use valet::Lot;
    use valet::SendHandler;
    use valet::db::Database;
    use valet::lot::DEFAULT_LOT;
    use valet::protocol::EmbeddedHandler;
    use valet::protocol::message::Unlock;
    use valet::user::User;

    let dir = tempfile::tempdir().unwrap();
    // Build the DB up front so we can register the user and seed the
    // default lot before the handler takes ownership of it.
    let db = Database::open_dir(dir.path()).await.expect("open db");
    let user = User::new(username, password.try_into().unwrap())
        .expect("new user")
        .register(&db)
        .await
        .expect("register user");
    Lot::new(DEFAULT_LOT, dir.path())
        .expect("new lot")
        .save(&db, &user)
        .await
        .expect("create default lot");
    let handler = EmbeddedHandler::new(
        db,
        dir.path().to_path_buf(),
        &tokio::runtime::Handle::current(),
    );
    handler
        .call(Unlock {
            username: username.to_owned(),
            password: password.try_into().unwrap(),
        })
        .await
        .expect("unlock");
    EmbeddedTestHandler {
        handler,
        _dir: dir,
    }
}

/// Fresh short-path temp directory for Unix-socket endpoints. Returns
/// a unique subdirectory under `/tmp` (not `std::env::temp_dir()`,
/// which on macOS returns a `/var/folders/...` path long enough to
/// blow past `AF_UNIX`'s `SUN_LEN` once you append a filename). Not
/// cleaned up. Fine for the test lifetime, and the path is
/// nanosecond-unique so runs don't collide.
#[cfg(feature = "protocol-socket")]
pub fn tempdir() -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir =
        std::path::PathBuf::from("/tmp").join(format!("valet-rt-{}-{}", std::process::id(), nanos));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
