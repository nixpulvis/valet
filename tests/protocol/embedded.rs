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
    Register, RemoteAdd, RemoteList, RemoteRemove, Status, Sync, Unlock,
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
    use valet::protocol::EmbeddedHandler;

    // Local-scoped tempdir: the handler only needs the path to live
    // through the test body, which it does by virtue of `dir` being
    // dropped after the last `.await` returns.
    let dir = tempfile::tempdir().unwrap();
    let client = EmbeddedHandler::open(dir.path(), &tokio::runtime::Handle::current())
        .await
        .unwrap();
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

#[tokio::test(flavor = "multi_thread")]
async fn remote_add_list_remove_roundtrip() {
    let client = embedded_client_with_user("alice", "sesame").await;
    let lot = valet::lot::DEFAULT_LOT;

    let add = client
        .call(RemoteAdd {
            username: "alice".into(),
            name: "origin".into(),
            url: "file:///tmp/valet-int-remote".into(),
            lots: vec![lot.into()],
        })
        .await
        .unwrap();
    assert_eq!(add.len(), 1);
    assert_eq!(add[0].0, lot);
    add[0].1.as_ref().unwrap();

    let listed = client
        .call(RemoteList {
            username: "alice".into(),
            lots: vec![],
        })
        .await
        .unwrap();
    let (lot_name, remotes) = listed.into_iter().find(|(n, _)| n == lot).unwrap();
    assert_eq!(lot_name, lot);
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "origin");
    // resolve_remote_url canonicalizes a generic file:// path to
    // <path>/parent.git for the auto-init shared-remote case.
    assert_eq!(remotes[0].url, "file:///tmp/valet-int-remote/parent.git");

    let remove = client
        .call(RemoteRemove {
            username: "alice".into(),
            name: "origin".into(),
            lots: vec![lot.into()],
        })
        .await
        .unwrap();
    remove[0].1.as_ref().unwrap();

    let after = client
        .call(RemoteList {
            username: "alice".into(),
            lots: vec![lot.into()],
        })
        .await
        .unwrap();
    assert!(after[0].1.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn remote_add_duplicate_errors_per_lot() {
    let client = embedded_client_with_user("alice", "sesame").await;
    let lot = valet::lot::DEFAULT_LOT;
    client
        .call(RemoteAdd {
            username: "alice".into(),
            name: "origin".into(),
            url: "file:///tmp/a".into(),
            lots: vec![lot.into()],
        })
        .await
        .unwrap();
    let again = client
        .call(RemoteAdd {
            username: "alice".into(),
            name: "origin".into(),
            url: "file:///tmp/b".into(),
            lots: vec![lot.into()],
        })
        .await
        .unwrap();
    assert!(again[0].1.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn sync_with_no_remotes_returns_empty() {
    let client = embedded_client_with_user("alice", "sesame").await;
    let outcomes = client
        .call(Sync {
            username: "alice".into(),
            lots: vec![],
        })
        .await
        .unwrap();
    assert!(outcomes.is_empty());
}

/// End-to-end sync round-trips: two embedded handlers backed by
/// separate data dirs, sharing a lot via cp-bootstrap (proper
/// lot-key sharing UX is a separate changeset). Drives the same
/// surface the CLI uses, then re-opens the receiving side fresh and
/// confirms records propagate.
mod sync_round_trip {
    use std::path::Path;
    use valet::SendHandler;
    use valet::protocol::EmbeddedHandler;
    use valet::protocol::message::{
        CreateLot, CreateRecord, Fetch, History, List, Register, RemoteAdd, Sync, SyncOutcome,
        SyncReport, Unlock,
    };
    use valet::record::Label;

    const PASSWORD: &str = "password";
    const USER: &str = "alice";
    const LOT: &str = "main";
    const RECORD: &str = "a";

    async fn open(data_dir: &Path) -> EmbeddedHandler {
        EmbeddedHandler::open(data_dir, &tokio::runtime::Handle::current())
            .await
            .unwrap()
    }

    async fn unlock(h: &EmbeddedHandler) {
        h.call(Unlock {
            username: USER.into(),
            password: PASSWORD.try_into().unwrap(),
        })
        .await
        .unwrap();
    }

    async fn register(h: &EmbeddedHandler) {
        h.call(Register {
            username: USER.into(),
            password: PASSWORD.try_into().unwrap(),
        })
        .await
        .unwrap();
    }

    async fn put(h: &EmbeddedHandler, password: &str) {
        h.call(CreateRecord {
            username: USER.into(),
            lot: LOT.into(),
            label: RECORD.parse::<Label>().unwrap(),
            password: password.try_into().unwrap(),
            extra: Default::default(),
        })
        .await
        .unwrap();
    }

    async fn add_remote(h: &EmbeddedHandler, name: &str, url: &str) {
        let res = h
            .call(RemoteAdd {
                username: USER.into(),
                name: name.into(),
                url: url.into(),
                lots: vec![LOT.into()],
            })
            .await
            .unwrap();
        assert_eq!(res.len(), 1);
        res[0].1.as_ref().expect("remote add");
    }

    async fn sync(h: &EmbeddedHandler) {
        let outcomes = h
            .call(Sync {
                username: USER.into(),
                lots: vec![LOT.into()],
            })
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1, "expected one (lot, remote) outcome");
        match &outcomes[0] {
            SyncOutcome {
                result: Ok(SyncReport::Clean { pushed: Ok(()), .. }),
                ..
            } => {}
            other => panic!("expected clean+pushed, got {other:?}"),
        }
    }

    async fn read_password(h: &EmbeddedHandler) -> String {
        let entries = h
            .call(List {
                username: USER.into(),
                queries: vec![format!("{LOT}::{RECORD}")],
            })
            .await
            .unwrap();
        assert_eq!(entries.len(), 1, "expected one record on read");
        let (uuid, _) = entries.into_iter().next().unwrap();
        let rec = h
            .call(Fetch {
                username: USER.into(),
                uuid,
            })
            .await
            .unwrap();
        rec.password().to_string()
    }

    fn copy_dir(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let p = entry.path();
            let to = dst.join(entry.file_name());
            if p.is_dir() {
                copy_dir(&p, &to);
            } else {
                std::fs::copy(&p, &to).unwrap();
            }
        }
    }

    /// Cp-bootstrap b from a, update on a, sync, b sees update.
    #[tokio::test(flavor = "multi_thread")]
    async fn propagates_update_after_cp_bootstrap() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        let h_a = open(dir_a.path()).await;
        register(&h_a).await;
        put(&h_a, "1").await;
        drop(h_a);

        copy_dir(dir_a.path(), dir_b.path());

        let h_a = open(dir_a.path()).await;
        unlock(&h_a).await;
        put(&h_a, "2").await;
        add_remote(&h_a, "b", &format!("file://{}", dir_b.path().display())).await;
        sync(&h_a).await;
        drop(h_a);

        let h_b = open(dir_b.path()).await;
        unlock(&h_b).await;
        assert_eq!(read_password(&h_b).await, "2");
    }

    /// Two pushes back-to-back: catches the failure mode where the
    /// second push advertises a parent commit referencing module
    /// oids the remote never received.
    #[tokio::test(flavor = "multi_thread")]
    async fn round_trips_across_two_pushes() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        let h_a = open(dir_a.path()).await;
        register(&h_a).await;
        put(&h_a, "1").await;
        drop(h_a);

        copy_dir(dir_a.path(), dir_b.path());

        let h_a = open(dir_a.path()).await;
        unlock(&h_a).await;
        add_remote(&h_a, "b", &format!("file://{}", dir_b.path().display())).await;
        sync(&h_a).await;

        put(&h_a, "2").await;
        sync(&h_a).await;
        drop(h_a);

        let h_b = open(dir_b.path()).await;
        unlock(&h_b).await;
        assert_eq!(read_password(&h_b).await, "2");
    }

    /// `Sync { lots: vec![] }` should iterate every lot the user has
    /// access to. Two lots, each with its own remote: outcomes
    /// returns one entry per lot.
    #[tokio::test(flavor = "multi_thread")]
    async fn defaults_to_all_user_lots_when_lots_empty() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_remote = tempfile::tempdir().unwrap();

        let h_a = open(dir_a.path()).await;
        register(&h_a).await;
        h_a.call(CreateLot {
            username: USER.into(),
            lot: "extra".into(),
        })
        .await
        .unwrap();
        // Put a record in each lot so both have a non-empty parent
        // ref to push.
        put(&h_a, "1").await;
        h_a.call(CreateRecord {
            username: USER.into(),
            lot: "extra".into(),
            label: "x".parse::<Label>().unwrap(),
            password: "1".try_into().unwrap(),
            extra: Default::default(),
        })
        .await
        .unwrap();
        drop(h_a);

        // Bootstrap the remote dir from a so the resolver finds both
        // lots' parent.git layouts under it.
        copy_dir(dir_a.path(), dir_remote.path());

        let h_a = open(dir_a.path()).await;
        unlock(&h_a).await;

        // Same URL on both lots: the resolver rewrites it per-lot to
        // `<remote>/lots/<our-lot-uuid>/repo/parent.git`.
        let url = format!("file://{}", dir_remote.path().display());
        add_remote(&h_a, "peer", &url).await;
        let res = h_a
            .call(RemoteAdd {
                username: USER.into(),
                name: "peer".into(),
                url: url.clone(),
                lots: vec!["extra".into()],
            })
            .await
            .unwrap();
        res[0].1.as_ref().expect("remote add on extra");

        let outcomes = h_a
            .call(Sync {
                username: USER.into(),
                lots: vec![],
            })
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 2, "expected one outcome per lot");
        let mut by_lot: Vec<&str> = outcomes.iter().map(|o| o.lot.as_str()).collect();
        by_lot.sort();
        assert_eq!(by_lot, vec!["extra", "main"]);
        for o in &outcomes {
            assert!(
                matches!(&o.result, Ok(SyncReport::Clean { pushed: Ok(()), .. })),
                "lot {} did not sync cleanly: {:?}",
                o.lot,
                o.result,
            );
        }
    }

    /// A lot with multiple configured remotes produces one
    /// SyncOutcome per (lot, remote) pair, all clean+pushed.
    #[tokio::test(flavor = "multi_thread")]
    async fn multiple_remotes_per_lot_each_get_an_outcome() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let dir_c = tempfile::tempdir().unwrap();

        let h_a = open(dir_a.path()).await;
        register(&h_a).await;
        put(&h_a, "1").await;
        drop(h_a);

        copy_dir(dir_a.path(), dir_b.path());
        copy_dir(dir_a.path(), dir_c.path());

        let h_a = open(dir_a.path()).await;
        unlock(&h_a).await;
        add_remote(&h_a, "b", &format!("file://{}", dir_b.path().display())).await;
        add_remote(&h_a, "c", &format!("file://{}", dir_c.path().display())).await;

        put(&h_a, "2").await;
        let outcomes = h_a
            .call(Sync {
                username: USER.into(),
                lots: vec![LOT.into()],
            })
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 2, "expected one outcome per (lot, remote)");
        let mut remotes: Vec<&str> = outcomes.iter().map(|o| o.remote.as_str()).collect();
        remotes.sort();
        assert_eq!(remotes, vec!["b", "c"]);
        for o in &outcomes {
            assert!(
                matches!(&o.result, Ok(SyncReport::Clean { pushed: Ok(()), .. })),
                "(main, {}) did not sync cleanly: {:?}",
                o.remote,
                o.result,
            );
        }
        drop(h_a);

        // Both peers should see "2" on read.
        for dir in [dir_b.path(), dir_c.path()] {
            let h = open(dir).await;
            unlock(&h).await;
            assert_eq!(
                read_password(&h).await,
                "2",
                "peer {dir:?} did not receive the push"
            );
        }
    }

    /// After a syncs two revisions of a record to b, b's `History`
    /// should report both revisions in newest-first order.
    /// Confirms storgit per-module history blobs propagate through
    /// push, not just the current value.
    #[tokio::test(flavor = "multi_thread")]
    async fn history_propagates_through_sync() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        let h_a = open(dir_a.path()).await;
        register(&h_a).await;
        put(&h_a, "1").await;
        drop(h_a);

        copy_dir(dir_a.path(), dir_b.path());

        let h_a = open(dir_a.path()).await;
        unlock(&h_a).await;
        put(&h_a, "2").await;
        add_remote(&h_a, "b", &format!("file://{}", dir_b.path().display())).await;
        sync(&h_a).await;
        drop(h_a);

        let h_b = open(dir_b.path()).await;
        unlock(&h_b).await;

        let entries = h_b
            .call(List {
                username: USER.into(),
                queries: vec![format!("{LOT}::{RECORD}")],
            })
            .await
            .unwrap();
        let (uuid, _) = entries.into_iter().next().unwrap();
        let revs = h_b
            .call(History {
                username: USER.into(),
                lot: LOT.into(),
                uuid,
            })
            .await
            .unwrap();
        assert_eq!(revs.len(), 2, "expected two revisions on b");
        // Newest-first: the "2" revision precedes the "1" revision.
        assert_eq!(revs[0].password.to_string(), "2");
        assert_eq!(revs[1].password.to_string(), "1");
    }

    /// Two cp-bootstrapped siblings each put different values for
    /// the same label, then a syncs with b as the remote. The pull
    /// must surface as `SyncReport::Conflicted` (no push, no
    /// silent-overwrite).
    #[tokio::test(flavor = "multi_thread")]
    async fn divergent_writes_surface_as_conflicted() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();

        let h_a = open(dir_a.path()).await;
        register(&h_a).await;
        put(&h_a, "initial").await;
        drop(h_a);

        copy_dir(dir_a.path(), dir_b.path());

        // Diverge: a writes "from_a", b writes "from_b".
        let h_a = open(dir_a.path()).await;
        unlock(&h_a).await;
        put(&h_a, "from_a").await;
        drop(h_a);

        let h_b = open(dir_b.path()).await;
        unlock(&h_b).await;
        put(&h_b, "from_b").await;
        drop(h_b);

        // Sync from a with b as remote: pull should fail to merge
        // cleanly because the same record was modified on both sides
        // since the common ancestor.
        let h_a = open(dir_a.path()).await;
        unlock(&h_a).await;
        add_remote(&h_a, "b", &format!("file://{}", dir_b.path().display())).await;
        let outcomes = h_a
            .call(Sync {
                username: USER.into(),
                lots: vec![LOT.into()],
            })
            .await
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        match &outcomes[0].result {
            Ok(SyncReport::Conflicted { conflicts }) => {
                assert!(!conflicts.is_empty(), "expected at least one conflict id");
            }
            other => panic!("expected Conflicted, got {other:?}"),
        }
    }
}
