//! Subdir-layout-specific tests: the on-disk tree shape
//! (`records/<id>/{data,label}`), the single-ref commit graph, and
//! path-scoped history semantics. Layout-agnostic behaviour lives in
//! `generic.rs` and runs against this layout there too.

mod common;

use common::{Handle, mkid, put_data};
use storgit::layout::subdir::SubdirLayout;

fn open() -> Handle<SubdirLayout> {
    common::make_subdir_store()
}

#[test]
fn delete_behaves_like_archive() {
    // SubdirLayout documents that `delete` is currently equivalent
    // to `archive` -- a single-ref layout can't cheaply erase one
    // record's history. Assert that history survives a delete so the
    // contract doesn't silently regress.
    let mut store = open();
    put_data(&mut store, "alpha", b"v1");
    store.delete(&mkid("alpha")).unwrap();

    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&mkid("alpha")).unwrap().is_none());

    let history = store.history(&mkid("alpha")).unwrap();
    assert_eq!(
        history.len(),
        2,
        "delete leaves put + archive commits reachable from HEAD",
    );
    assert_eq!(history[0].data, None, "archive commit has no payload");
    assert_eq!(history[1].data.as_deref(), Some(&b"v1"[..]));
}

#[test]
fn history_is_path_scoped() {
    // Subdir shares one ref across all records, but history() must
    // only surface commits that touched the given record's subtree.
    // If puts to beta leaked into alpha's history, this would break.
    let mut store = open();
    put_data(&mut store, "alpha", b"a1");
    put_data(&mut store, "beta", b"b1");
    put_data(&mut store, "alpha", b"a2");
    put_data(&mut store, "beta", b"b2");
    put_data(&mut store, "beta", b"b3");

    let alpha = store.history(&mkid("alpha")).unwrap();
    let alpha_data: Vec<Option<&[u8]>> = alpha.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(alpha_data, vec![Some(&b"a2"[..]), Some(&b"a1"[..])]);

    let beta = store.history(&mkid("beta")).unwrap();
    let beta_data: Vec<Option<&[u8]>> = beta.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(
        beta_data,
        vec![Some(&b"b3"[..]), Some(&b"b2"[..]), Some(&b"b1"[..])]
    );
}

#[test]
fn archive_is_visible_only_in_archived_records_history() {
    // An archive of alpha must not show up as a tombstone in beta's
    // history, even though it advanced the shared ref.
    let mut store = open();
    put_data(&mut store, "alpha", b"a");
    put_data(&mut store, "beta", b"b");
    store.archive(&mkid("alpha")).unwrap();

    let beta = store.history(&mkid("beta")).unwrap();
    let beta_data: Vec<Option<&[u8]>> = beta.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(
        beta_data,
        vec![Some(&b"b"[..])],
        "archive of an unrelated record must not appear in beta's path-scoped log",
    );

    let alpha = store.history(&mkid("alpha")).unwrap();
    assert_eq!(alpha.len(), 2, "alpha has put + archive tombstone");
    assert_eq!(alpha[0].data, None);
    assert_eq!(alpha[1].data.as_deref(), Some(&b"a"[..]));
}

#[test]
fn writes_to_one_entry_do_not_rewrite_other_entries_history() {
    // Parallel of the submodule test `snapshot_only_reports_touched_modules`,
    // expressed through path-scoped history: after a put on alpha,
    // beta's history must still be exactly its earlier single commit.
    let mut store = open();
    put_data(&mut store, "alpha", b"1");
    put_data(&mut store, "beta", b"1");
    let before: Vec<storgit::CommitId> = store
        .history(&mkid("beta"))
        .unwrap()
        .into_iter()
        .map(|e| e.commit)
        .collect();
    put_data(&mut store, "alpha", b"2");
    let after: Vec<storgit::CommitId> = store
        .history(&mkid("beta"))
        .unwrap()
        .into_iter()
        .map(|e| e.commit)
        .collect();
    assert_eq!(
        before, after,
        "beta's path-scoped history must not change when alpha is written",
    );
}

#[test]
fn put_is_noop_detection_works_across_interleaved_writes() {
    // In subdir the no-op check compares records/<id>/ subtree oids,
    // not the whole commit tree. A write to beta must not make an
    // identical re-put of alpha look non-identical (which it would
    // if no-op compared at the root-tree level).
    let mut store = open();
    store.put(&mkid("alpha"), Some(b"m"), Some(b"x")).unwrap();
    put_data(&mut store, "beta", b"unrelated");
    assert!(
        store
            .put(&mkid("alpha"), Some(b"m"), Some(b"x"))
            .unwrap()
            .is_none(),
        "identical alpha put is still a no-op even after an intervening beta write",
    );
}
