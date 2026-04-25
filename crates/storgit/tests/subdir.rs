//! Subdir-layout-specific tests: the on-disk tree shape
//! (`records/<id>/{data,label}`), the single-ref commit graph, and
//! path-scoped history semantics. Layout-agnostic behaviour lives in
//! `generic.rs` and runs against this layout there too.

mod common;

use common::{Handle, mkid, put_data};
use storgit::layout::Layout;
use storgit::layout::subdir::SubdirLayout;
use storgit::merge::Merge;

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

// --- Merge tests (subdir layout) -----------------------------------

use storgit::merge::{MergeStatus, Side};

fn pull_url(store: &Handle<SubdirLayout>) -> String {
    format!("file://{}", store.git_dir().display())
}

#[test]
fn pull_no_op_on_identical_state() {
    let mut a = open();
    let b = open();
    a.add_remote("b", &pull_url(&b)).unwrap();
    let status = a.pull("b").unwrap();
    match status {
        MergeStatus::Clean(applied) => assert!(applied.is_empty()),
        _ => panic!("expected clean no-op"),
    }
}

#[test]
fn pull_loads_into_empty_store() {
    let mut src = open();
    put_data(&mut src, "alpha", b"hi");
    let mut dst = open();
    dst.add_remote("src", &pull_url(&src)).unwrap();
    let status = dst.pull("src").unwrap();
    match status {
        MergeStatus::Clean(applied) => assert!(!applied.is_empty()),
        _ => panic!("expected clean"),
    }
    assert_eq!(
        dst.get(&mkid("alpha")).unwrap().and_then(|e| e.data),
        Some(b"hi".to_vec())
    );
}

#[test]
fn pull_fast_forwards_when_local_is_ancestor() {
    let mut src = open();
    put_data(&mut src, "alpha", b"v1");
    let mut dst = open();
    dst.add_remote("src", &pull_url(&src)).unwrap();
    dst.pull("src").unwrap();
    // Now src advances.
    put_data(&mut src, "alpha", b"v2");
    let status = dst.pull("src").unwrap();
    match status {
        MergeStatus::Clean(applied) => assert!(!applied.is_empty()),
        _ => panic!("expected ff"),
    }
    assert_eq!(
        dst.get(&mkid("alpha")).unwrap().and_then(|e| e.data),
        Some(b"v2".to_vec())
    );
}

#[test]
fn pull_clean_3way_when_disjoint_ids() {
    let mut a = open();
    let mut b = open();
    put_data(&mut a, "alpha", b"a");
    a.add_remote("b", &pull_url(&b)).unwrap();
    a.pull("b").unwrap(); // a takes empty b -> no-op
    put_data(&mut b, "beta", b"b");
    let status = a.pull("b").unwrap();
    // a has alpha, b has beta -> clean merge
    let _ = match status {
        MergeStatus::Clean(applied) => applied,
        MergeStatus::Conflicted(_) => {
            panic!("expected clean merge for disjoint ids");
        }
    };
    assert_eq!(
        a.get(&mkid("alpha")).unwrap().and_then(|e| e.data),
        Some(b"a".to_vec())
    );
    assert_eq!(
        a.get(&mkid("beta")).unwrap().and_then(|e| e.data),
        Some(b"b".to_vec())
    );
}

#[test]
fn pull_conflict_then_resolve_local() {
    let mut a = open();
    let mut b = open();
    put_data(&mut a, "alpha", b"shared");
    a.add_remote("b", &pull_url(&b)).unwrap();
    b.add_remote("a", &pull_url(&a)).unwrap();
    b.pull("a").unwrap(); // b is now at a's state

    // Diverge.
    put_data(&mut a, "alpha", b"a-version");
    put_data(&mut b, "alpha", b"b-version");

    let status = a.pull("b").unwrap();
    let mut progress = match status {
        MergeStatus::Conflicted(p) => p,
        _ => panic!("expected conflicts"),
    };
    assert!(a.merge_in_progress());
    assert_eq!(progress.conflicts().len(), 1);
    assert_eq!(progress.conflicts()[0].id.as_str(), "alpha");

    progress.pick(&mkid("alpha"), Side::Local).unwrap();
    let resolution = progress.resolve().ok().unwrap();
    a.merge(resolution).unwrap();
    assert!(!a.merge_in_progress());

    assert_eq!(
        a.get(&mkid("alpha")).unwrap().and_then(|e| e.data),
        Some(b"a-version".to_vec())
    );
}

#[test]
fn pull_conflict_then_resolve_incoming() {
    let mut a = open();
    let mut b = open();
    put_data(&mut a, "alpha", b"shared");
    a.add_remote("b", &pull_url(&b)).unwrap();
    b.add_remote("a", &pull_url(&a)).unwrap();
    b.pull("a").unwrap();

    put_data(&mut a, "alpha", b"a-version");
    put_data(&mut b, "alpha", b"b-version");

    let status = a.pull("b").unwrap();
    let mut progress = match status {
        MergeStatus::Conflicted(p) => p,
        _ => panic!("expected conflicts"),
    };
    progress.pick(&mkid("alpha"), Side::Incoming).unwrap();
    let resolution = progress.resolve().ok().unwrap();
    a.merge(resolution).unwrap();

    assert_eq!(
        a.get(&mkid("alpha")).unwrap().and_then(|e| e.data),
        Some(b"b-version".to_vec())
    );
}

#[test]
fn put_during_merge_errors_subdir() {
    let mut a = open();
    let mut b = open();
    put_data(&mut a, "alpha", b"shared");
    a.add_remote("b", &pull_url(&b)).unwrap();
    b.add_remote("a", &pull_url(&a)).unwrap();
    b.pull("a").unwrap();

    put_data(&mut a, "alpha", b"a-v");
    put_data(&mut b, "alpha", b"b-v");

    let status = a.pull("b").unwrap();
    assert!(matches!(status, MergeStatus::Conflicted(_)));
    assert!(a.put(&mkid("alpha"), None, Some(b"x")).is_err());
}

#[test]
fn abort_clears_merge_state() {
    let mut a = open();
    let mut b = open();
    put_data(&mut a, "alpha", b"shared");
    a.add_remote("b", &pull_url(&b)).unwrap();
    b.add_remote("a", &pull_url(&a)).unwrap();
    b.pull("a").unwrap();
    put_data(&mut a, "alpha", b"a-v");
    put_data(&mut b, "alpha", b"b-v");

    let status = a.pull("b").unwrap();
    assert!(matches!(status, MergeStatus::Conflicted(_)));
    assert!(a.merge_in_progress());
    a.abort().unwrap();
    assert!(!a.merge_in_progress());
    // After abort, put works again.
    a.put(&mkid("alpha"), None, Some(b"new")).unwrap();
}

#[test]
fn merge_creates_two_parent_commit() {
    let mut a = open();
    let mut b = open();
    put_data(&mut a, "alpha", b"shared");
    a.add_remote("b", &pull_url(&b)).unwrap();
    b.add_remote("a", &pull_url(&a)).unwrap();
    b.pull("a").unwrap();

    put_data(&mut a, "alpha", b"a-v");
    put_data(&mut b, "beta", b"b-v");

    let status = a.pull("b").unwrap();
    if let MergeStatus::Conflicted(_) = status {
        panic!("disjoint ids should not conflict");
    }
    // Walk HEAD's commit, verify two parents.
    let repo = gix::open(a.git_dir()).unwrap();
    let head_oid = std::fs::read_to_string(a.git_dir().join("refs/heads/main"))
        .unwrap()
        .trim()
        .to_string();
    let head_oid = gix::ObjectId::from_hex(head_oid.as_bytes()).unwrap();
    let commit = repo.find_object(head_oid).unwrap().into_commit();
    let parents: Vec<_> = commit.decode().unwrap().parents().collect();
    assert_eq!(parents.len(), 2, "merge commit must have two parents");
}
