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

/// Register a user, create the default lot, unlock, and return the
/// resulting [`EmbeddedHandler`]. Every embedded / socket / native-
/// messaging test starts from this state.
///
/// [`EmbeddedHandler`]: valet::protocol::EmbeddedHandler
#[cfg(feature = "protocol-embedded")]
pub async fn embedded_client_with_user(
    username: &str,
    password: &str,
) -> valet::protocol::EmbeddedHandler {
    use valet::Lot;
    use valet::SendHandler;
    use valet::db::Database;
    use valet::lot::DEFAULT_LOT;
    use valet::protocol::EmbeddedHandler;
    use valet::protocol::message::Unlock;
    use valet::user::User;

    let db = Database::new("sqlite://:memory:")
        .await
        .expect("open in-memory db");
    let user = User::new(username, password.try_into().unwrap())
        .expect("new user")
        .register(&db)
        .await
        .expect("register user");
    // `EmbeddedHandler::new` takes ownership of the DB, so create the
    // default lot before handing the DB over.
    Lot::new(DEFAULT_LOT)
        .save(&db, &user)
        .await
        .expect("create default lot");
    let client = EmbeddedHandler::new(db, &tokio::runtime::Handle::current());
    client
        .call(Unlock {
            username: username.to_owned(),
            password: password.try_into().unwrap(),
        })
        .await
        .expect("unlock");
    client
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
