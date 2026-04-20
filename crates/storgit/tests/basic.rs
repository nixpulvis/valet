use std::collections::HashMap;

use storgit::{CommitId, Entry, Id, IdError, ModuleChange, Parts, Store};

fn empty() -> Parts {
    Parts {
        parent: Vec::new(),
        modules: HashMap::new(),
    }
}

/// Shorthand for building an [`Id`] in-test. Panics on invalid input —
/// tests only pass known-good strings, and a bad id is a bug in the
/// test, not behaviour we want to silently ignore.
fn id(s: &str) -> Id {
    Id::new(s).unwrap()
}

/// Shorthand for "put data only, no label"; most tests don't care
/// about the label slot.
fn put_data(store: &mut Store, id_str: &str, data: &[u8]) {
    store.put(&id(id_str), None, Some(data)).unwrap();
}

/// Extract just the `data` slot of the current [`Entry`] for `id`,
/// or `None` if the id is not a live entry.
fn get_data(store: &Store, id_str: &str) -> Option<Vec<u8>> {
    store.get(&id(id_str)).unwrap().and_then(|e| e.data)
}

#[test]
fn open_empty_roundtrips() {
    let mut store = Store::open(empty()).expect("open empty");
    let snap = store.snapshot().expect("snapshot");
    assert!(snap.parent.is_some(), "fresh store must publish its parent");
    assert!(snap.modules.is_empty());
    let mut parts = empty();
    parts.apply(snap);
    Store::open(parts).expect("reopen");
}

#[test]
fn put_then_get_returns_latest() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"one");
    put_data(&mut store, "alpha", b"two");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"two"[..]));
}

#[test]
fn get_missing_entry_returns_none() {
    let store = Store::open(empty()).unwrap();
    assert!(store.get(&id("nope")).unwrap().is_none());
}

#[test]
fn put_roundtrips_through_parts() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"hello");
    parts.apply(store.snapshot().unwrap());
    let reopened = Store::open(parts).unwrap();
    assert_eq!(get_data(&reopened, "alpha").as_deref(), Some(&b"hello"[..]));
}

#[test]
fn history_returns_all_versions_newest_first() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"v1");
    put_data(&mut store, "alpha", b"v2");
    put_data(&mut store, "alpha", b"v3");
    let history = store.history(&id("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(
        payloads,
        vec![Some(&b"v3"[..]), Some(&b"v2"[..]), Some(&b"v1"[..])]
    );
}

#[test]
fn list_names_live_entries() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "a", b"x");
    put_data(&mut store, "b", b"y");
    let mut ids = store.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![id("a"), id("b")]);
}

#[test]
fn archive_removes_from_list_but_history_survives() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "gone", b"bye");
    store.archive(&id("gone")).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&id("gone")).unwrap().is_none());
    let history = store.history(&id("gone")).unwrap();
    assert_eq!(history.len(), 2, "archive appends a tombstone commit");
    assert_eq!(history[0].data, None, "newest commit is the tombstone");
    assert_eq!(history[0].label, None);
    assert_eq!(history[1].data.as_deref(), Some(&b"bye"[..]));
}

#[test]
fn re_put_after_archive_continues_submodule_history() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"v1");
    store.archive(&id("alpha")).unwrap();
    put_data(&mut store, "alpha", b"v2");
    let history = store.history(&id("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(
        payloads,
        vec![Some(&b"v2"[..]), None, Some(&b"v1"[..])],
        "history is put -> tombstone -> put, newest first",
    );
}

#[test]
fn delete_drops_submodule_and_history() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "gone", b"bye");
    store.delete(&id("gone")).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&id("gone")).unwrap().is_none());
    assert!(store.history(&id("gone")).unwrap().is_empty());
}

#[test]
fn re_put_after_delete_starts_fresh_history() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"v1");
    store.delete(&id("alpha")).unwrap();
    put_data(&mut store, "alpha", b"v2");
    let history = store.history(&id("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..])]);
}

#[test]
fn put_returns_matching_commit_id_for_latest_history_entry() {
    let mut store = Store::open(empty()).unwrap();
    let commit: CommitId = store
        .put(&id("alpha"), None, Some(b"payload"))
        .unwrap()
        .expect("first put writes a commit");
    let history = store.history(&id("alpha")).unwrap();
    assert_eq!(history.first().map(|e| &e.commit), Some(&commit));
}

#[test]
fn empty_payload_roundtrips() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b""[..]));
}

#[test]
fn history_survives_parts_roundtrip() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"v1");
    put_data(&mut store, "alpha", b"v2");
    parts.apply(store.snapshot().unwrap());
    let reopened = Store::open(parts).unwrap();
    let history = reopened.history(&id("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..]), Some(&b"v1"[..])]);
}

#[test]
fn snapshot_only_reports_touched_modules() {
    // The whole point of the split: writing one entry must not mark any
    // other entry's tarball as dirty.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"1");
    put_data(&mut store, "beta", b"1");
    let _first = store.snapshot().unwrap();
    put_data(&mut store, "alpha", b"2");
    let second = store.snapshot().unwrap();
    assert!(second.parent.is_some(), "parent advances on every put");
    assert!(second.modules.contains_key(&id("alpha")));
    assert!(
        !second.modules.contains_key(&id("beta")),
        "beta was untouched and must not reappear in the snapshot"
    );
}

#[test]
fn snapshot_is_empty_when_nothing_changed() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"1");
    let _first = store.snapshot().unwrap();
    let second = store.snapshot().unwrap();
    assert!(second.parent.is_none());
    assert!(second.modules.is_empty());
}

#[test]
fn save_load_roundtrips_all_state() {
    // The self-contained save/load path: one tarball in, one tarball
    // back out, all entries and their history preserved.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"a1");
    put_data(&mut store, "alpha", b"a2");
    put_data(&mut store, "beta", b"b1");
    let bytes = store.save().unwrap();

    let reloaded = Store::load(&bytes).unwrap();
    let mut ids = reloaded.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![id("alpha"), id("beta")]);
    assert_eq!(get_data(&reloaded, "alpha").as_deref(), Some(&b"a2"[..]));
    let history = reloaded.history(&id("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"a2"[..]), Some(&b"a1"[..])]);
}

#[test]
fn save_is_nondestructive() {
    // save() flushes any pending parent commit, but the caller keeps
    // the handle and can keep writing afterward.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"1");
    let _bytes = store.save().unwrap();
    put_data(&mut store, "beta", b"2");
    let mut ids = store.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![id("alpha"), id("beta")]);
}

#[test]
fn save_bundles_every_module_even_ones_not_touched_since_last_snapshot() {
    // snapshot() reports incremental deltas; save() is the full bundle.
    // After clearing dirty state with a snapshot, save() must still
    // contain every module or a reload would lose data.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"a");
    put_data(&mut store, "beta", b"b");
    let _drain_dirty = store.snapshot().unwrap();
    let bytes = store.save().unwrap();

    let reloaded = Store::load(&bytes).unwrap();
    assert_eq!(get_data(&reloaded, "alpha").as_deref(), Some(&b"a"[..]));
    assert_eq!(get_data(&reloaded, "beta").as_deref(), Some(&b"b"[..]));
}

#[test]
fn delete_emits_module_deletion_in_snapshot() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"1");
    parts.apply(store.snapshot().unwrap());
    assert!(parts.modules.contains_key(&id("alpha")));
    store.delete(&id("alpha")).unwrap();
    let snap = store.snapshot().unwrap();
    assert!(matches!(
        snap.modules.get(&id("alpha")),
        Some(ModuleChange::Deleted)
    ));
    parts.apply(snap);
    assert!(!parts.modules.contains_key(&id("alpha")));
}

#[test]
fn fresh_module_stays_under_1kb_on_disk() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"payload");
    let bytes = store.save().unwrap();

    let tmp = tempfile::tempdir().unwrap();
    tar::Archive::new(std::io::Cursor::new(bytes))
        .unpack(tmp.path())
        .unwrap();
    let module = tmp.path().join("modules").join("alpha.git");
    let size = dir_size(&module).unwrap();
    assert!(
        size < 1024,
        "fresh submodule should be <1 KB on disk; got {size} B. \
         likely cause: init_bare templates (hooks/, info/, description) \
         are being shipped again."
    );
}

fn dir_size(path: &std::path::Path) -> std::io::Result<u64> {
    let mut total = 0;
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let md = entry.metadata()?;
        if md.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += md.len();
        }
    }
    Ok(total)
}

#[test]
fn parent_objects_stay_bounded_after_many_puts() {
    // parent.git holds only its own parent-tree+commit, the
    // .gitmodules blob, and the index/ subtree+blobs. With deferred
    // flushing there's exactly one parent commit at any time, and
    // superseded commits get pruned, so its loose-object count must
    // stay tiny regardless of how many entries the store holds.
    const N: usize = 50;
    let mut store = Store::open(empty()).unwrap();
    for i in 0..N {
        store
            .put(
                &id(&format!("entry-{i:04}")),
                None,
                Some(format!("payload-{i}").as_bytes()),
            )
            .unwrap();
    }
    let bytes = store.save().unwrap();

    let tmp = tempfile::tempdir().unwrap();
    tar::Archive::new(std::io::Cursor::new(bytes))
        .unpack(tmp.path())
        .unwrap();
    let parent_objects = tmp.path().join("parent.git").join("objects");
    let loose = count_loose_objects(&parent_objects);
    // Expect: 1 parent commit + 1 parent tree + 1 .gitmodules blob = 3.
    // No labels were set, so no index/ entries.
    assert!(
        loose <= 5,
        "parent.git has {loose} loose objects after {N} puts; \
         expected ~3 (parent commit + tree + .gitmodules)",
    );
}

fn count_loose_objects(objects_root: &std::path::Path) -> usize {
    let mut n = 0;
    let Ok(dir) = std::fs::read_dir(objects_root) else {
        return 0;
    };
    for entry in dir.flatten() {
        let fname = entry.file_name();
        let s = fname.to_string_lossy();
        if s.len() == 2 && s.chars().all(|c| c.is_ascii_hexdigit()) {
            if let Ok(sub) = std::fs::read_dir(entry.path()) {
                n += sub.flatten().count();
            }
        }
    }
    n
}

#[test]
fn parent_history_is_squashed_to_one_commit() {
    let mut store = Store::open(empty()).unwrap();
    for i in 0..50 {
        put_data(&mut store, &format!("entry-{i:04}"), b"x");
    }
    store.archive(&id("entry-0000")).unwrap();
    store.delete(&id("entry-0001")).unwrap();

    let bytes = store.save().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    tar::Archive::new(std::io::Cursor::new(bytes))
        .unpack(tmp.path())
        .unwrap();
    let parent = gix::open(tmp.path().join("parent.git")).unwrap();
    let head = parent.head_commit().unwrap();
    let count = head.ancestors().all().unwrap().count();
    assert_eq!(count, 1, "parent history should be squashed");
}

#[test]
fn new_store_is_empty_and_writable() {
    let mut store = Store::new().unwrap();
    assert!(store.list().unwrap().is_empty());
    put_data(&mut store, "alpha", b"x");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"x"[..]));
}

#[test]
fn parts_from_first_snapshot_can_be_reopened() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"hi");
    let parts: Parts = store.snapshot().unwrap().into();
    let reopened = Store::open(parts).unwrap();
    assert_eq!(get_data(&reopened, "alpha").as_deref(), Some(&b"hi"[..]));
}

#[test]
fn parent_ref_is_not_updated_between_puts_without_snapshot() {
    // Deferred parent materialisation: a batch of N puts produces
    // exactly one parent commit on the next snapshot, regardless of
    // how many puts went in. We verify by inspecting parent.git after
    // a snapshot — only one parent commit + tree should be present
    // (plus the .gitmodules blob), not N.
    let mut store = Store::open(empty()).unwrap();
    for i in 0..10 {
        put_data(&mut store, &format!("entry-{i:04}"), b"x");
    }
    let snap = store.snapshot().unwrap();
    let parent_bytes = snap.parent.expect("parent emitted on first snapshot");
    let tmp = tempfile::tempdir().unwrap();
    tar::Archive::new(std::io::Cursor::new(&parent_bytes))
        .unpack(tmp.path())
        .unwrap();
    let parent_objects = tmp.path().join("objects");
    let loose = count_loose_objects(&parent_objects);
    // 1 parent commit + 1 parent tree + 1 .gitmodules blob = 3.
    assert!(
        loose <= 5,
        "expected ~3 loose parent objects after 10 deferred puts; got {loose}",
    );
}

#[test]
fn parts_apply_merges_successive_snapshots() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"1");
    parts.apply(store.snapshot().unwrap());
    put_data(&mut store, "beta", b"2");
    parts.apply(store.snapshot().unwrap());
    let reopened = Store::open(parts).unwrap();
    let mut ids = reopened.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![id("alpha"), id("beta")]);
}

// --- label / data + label coverage --------------------------------------

#[test]
fn put_rejects_both_sides_none() {
    let mut store = Store::open(empty()).unwrap();
    assert!(store.put(&id("alpha"), None, None).is_err());
}

#[test]
fn put_label_and_data_roundtrips() {
    let mut store = Store::open(empty()).unwrap();
    store
        .put(&id("alpha"), Some(b"label"), Some(b"payload"))
        .unwrap();
    let entry: Entry = store.get(&id("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(entry.data.as_deref(), Some(&b"payload"[..]));
}

#[test]
fn put_none_slot_carries_prior_blob_forward() {
    let mut store = Store::open(empty()).unwrap();
    store.put(&id("alpha"), None, Some(b"payload")).unwrap();
    store.put(&id("alpha"), Some(b"label"), None).unwrap();

    let latest = store.get(&id("alpha")).unwrap().expect("live entry");
    assert_eq!(
        latest.data.as_deref(),
        Some(&b"payload"[..]),
        "None data reuses the prior commit's data blob"
    );
    assert_eq!(latest.label.as_deref(), Some(&b"label"[..]));

    let history = store.history(&id("alpha")).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].data.as_deref(), Some(&b"payload"[..]));
    assert_eq!(history[0].label.as_deref(), Some(&b"label"[..]));
    assert_eq!(history[1].data.as_deref(), Some(&b"payload"[..]));
    assert_eq!(history[1].label, None);
}

#[test]
fn put_label_only_is_noop_when_label_matches_prior() {
    let mut store = Store::open(empty()).unwrap();
    store
        .put(&id("alpha"), Some(b"label"), Some(b"payload"))
        .unwrap();
    assert!(
        store
            .put(&id("alpha"), Some(b"label"), None)
            .unwrap()
            .is_none(),
        "label-only put with unchanged label is a no-op (data reused)"
    );
    assert_eq!(store.history(&id("alpha")).unwrap().len(), 1);
}

#[test]
fn put_none_on_fresh_module_omits_slot() {
    let mut store = Store::open(empty()).unwrap();
    store.put(&id("alpha"), Some(b"label"), None).unwrap();
    let entry = store.get(&id("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(
        entry.data, None,
        "no prior commit to reuse, so the slot is omitted"
    );
}

#[test]
fn put_is_noop_when_tree_matches_head() {
    let mut store = Store::open(empty()).unwrap();
    assert!(
        store
            .put(&id("alpha"), Some(b"m"), Some(b"x"))
            .unwrap()
            .is_some(),
        "first put writes a commit"
    );
    assert!(
        store
            .put(&id("alpha"), Some(b"m"), Some(b"x"))
            .unwrap()
            .is_none(),
        "identical put is a no-op"
    );
    assert_eq!(
        store.history(&id("alpha")).unwrap().len(),
        1,
        "no second commit was written"
    );
}

#[test]
fn label_cache_surfaces_via_label_and_list_labels() {
    let mut store = Store::open(empty()).unwrap();
    store.put(&id("a"), Some(b"label-a"), Some(b"d1")).unwrap();
    store.put(&id("b"), Some(b"label-b"), Some(b"d2")).unwrap();
    store.put(&id("c"), None, Some(b"d3")).unwrap();

    assert_eq!(store.label(&id("a")), Some(&b"label-a"[..]));
    assert_eq!(store.label(&id("b")), Some(&b"label-b"[..]));
    assert_eq!(store.label(&id("c")), None, "no label set for c");
    assert_eq!(store.label(&id("missing")), None);

    let mut listed = store.list_labels();
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        listed,
        vec![
            (id("a"), b"label-a".to_vec()),
            (id("b"), b"label-b".to_vec()),
        ],
        "list_labels omits modules whose label is absent",
    );
}

#[test]
fn label_cache_survives_parts_roundtrip() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    store
        .put(&id("alpha"), Some(b"label"), Some(b"data"))
        .unwrap();
    parts.apply(store.snapshot().unwrap());

    let reopened = Store::open(parts).unwrap();
    assert_eq!(reopened.label(&id("alpha")), Some(&b"label"[..]));
    assert_eq!(
        reopened.list_labels(),
        vec![(id("alpha"), b"label".to_vec())],
    );
}

#[test]
fn empty_label_is_not_indexed_but_still_in_history() {
    let mut store = Store::open(empty()).unwrap();
    store.put(&id("alpha"), Some(b""), Some(b"data")).unwrap();
    assert_eq!(
        store.label(&id("alpha")),
        None,
        "empty label is not indexed"
    );
    assert!(store.list_labels().is_empty());

    let entry = store.get(&id("alpha")).unwrap().expect("live entry");
    assert_eq!(
        entry.label.as_deref(),
        Some(&b""[..]),
        "empty-bytes label is still recorded in the commit tree",
    );
}

#[test]
fn archive_clears_label_from_cache() {
    let mut store = Store::open(empty()).unwrap();
    store
        .put(&id("alpha"), Some(b"label"), Some(b"data"))
        .unwrap();
    assert_eq!(store.label(&id("alpha")), Some(&b"label"[..]));
    store.archive(&id("alpha")).unwrap();
    assert_eq!(store.label(&id("alpha")), None);
    assert!(store.list_labels().is_empty());
}

// --- Id validation ------------------------------------------------------

#[test]
fn id_rejects_empty() {
    assert_eq!(Id::new(""), Err(IdError::Empty));
}

#[test]
fn id_rejects_slash_and_nul() {
    assert_eq!(Id::new("a/b"), Err(IdError::BadChar('/')));
    assert_eq!(Id::new("a\0b"), Err(IdError::BadChar('\0')));
}

#[test]
fn id_rejects_quote_and_backslash() {
    // Both characters would need escaping inside the .gitmodules
    // section name, so storgit forbids them at the boundary instead.
    assert_eq!(Id::new("a\"b"), Err(IdError::BadChar('"')));
    assert_eq!(Id::new("a\\b"), Err(IdError::BadChar('\\')));
}

#[test]
fn id_rejects_control_chars() {
    assert_eq!(Id::new("a\nb"), Err(IdError::BadChar('\n')));
    assert_eq!(Id::new("a\tb"), Err(IdError::BadChar('\t')));
    assert_eq!(Id::new("a\rb"), Err(IdError::BadChar('\r')));
    assert_eq!(Id::new("a\x01b"), Err(IdError::BadChar('\x01')));
    assert_eq!(Id::new("a\x7fb"), Err(IdError::BadChar('\x7f')));
}

#[test]
fn id_rejects_leading_dot() {
    assert_eq!(Id::new(".foo"), Err(IdError::LeadingDot));
    assert_eq!(Id::new("."), Err(IdError::LeadingDot));
    assert_eq!(Id::new(".."), Err(IdError::LeadingDot));
}

#[test]
fn id_rejects_git_suffix() {
    assert_eq!(Id::new("foo.git"), Err(IdError::GitSuffix));
}

#[test]
fn id_rejects_reserved_names() {
    assert_eq!(Id::new("index"), Err(IdError::Reserved));
}

#[test]
fn id_rejects_too_long() {
    let long = "a".repeat(Id::MAX_LEN + 1);
    assert!(matches!(Id::new(long), Err(IdError::TooLong { .. })));
}

#[test]
fn id_accepts_reasonable_strings() {
    Id::new("alpha").unwrap();
    Id::new("alpha-beta").unwrap();
    Id::new("user@example.com").unwrap();
    Id::new("01945e9b-3e3f-7b2a-b8ab-8a52c82d4c01").unwrap();
}

// --- .gitmodules manifest ----------------------------------------------

/// Extract a saved store to a fresh tempdir and return the path so
/// individual tests can poke at the on-disk layout.
fn extract_to_tmp(bytes: &[u8]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    tar::Archive::new(std::io::Cursor::new(bytes))
        .unpack(tmp.path())
        .unwrap();
    tmp
}

#[test]
fn gitmodules_blob_appears_in_parent_tree_after_put() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"x");
    put_data(&mut store, "beta", b"y");
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let parent = gix::open(tmp.path().join("parent.git")).unwrap();
    let head = parent.head_commit().unwrap();
    let tree = head.tree().unwrap();
    let entry = tree
        .find_entry(".gitmodules")
        .expect(".gitmodules must be present in parent tree");
    let blob = parent.find_object(entry.oid()).unwrap();
    let content = std::str::from_utf8(&blob.data).unwrap().to_string();
    assert_eq!(
        content,
        concat!(
            "[submodule \"alpha\"]\n\tpath = alpha\n\turl = ../modules/alpha.git\n",
            "[submodule \"beta\"]\n\tpath = beta\n\turl = ../modules/beta.git\n",
        ),
    );
}

#[test]
fn gitmodules_omitted_when_no_live_entries() {
    // A fresh store with no puts has no submodules, so writing a
    // stub `.gitmodules` would be noise. The flush should skip it.
    let mut store = Store::open(empty()).unwrap();
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let parent_path = tmp.path().join("parent.git");
    // The parent has no commit yet (no mutations -> no flush), so the
    // blob simply isn't anywhere on disk. Check the dir doesn't even
    // hint at one; we don't want a stub `.gitmodules` blob in the ODB.
    let parent = gix::open(&parent_path).unwrap();
    assert!(parent.head_commit().is_err(), "fresh parent has no HEAD");
}

#[test]
fn gitmodules_updates_when_entry_is_archived() {
    // Archiving an entry removes it from the live gitlink set, so the
    // next flush should drop its `.gitmodules` stanza too.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "keep", b"x");
    put_data(&mut store, "drop", b"y");
    store.archive(&id("drop")).unwrap();
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let parent = gix::open(tmp.path().join("parent.git")).unwrap();
    let head = parent.head_commit().unwrap();
    let tree = head.tree().unwrap();
    let entry = tree.find_entry(".gitmodules").unwrap();
    let blob = parent.find_object(entry.oid()).unwrap();
    let content = std::str::from_utf8(&blob.data).unwrap().to_string();
    assert!(content.contains("[submodule \"keep\"]"));
    assert!(
        !content.contains("[submodule \"drop\"]"),
        "archived id must not stay in .gitmodules: {content}",
    );
}

#[test]
fn gitmodules_parses_as_git_submodule_config() {
    // Use gix to actually parse the file as git-config and confirm
    // each submodule's path/url come back through the standard parser.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"x");
    put_data(&mut store, "user@host.com", b"y");
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let parent = gix::open(tmp.path().join("parent.git")).unwrap();
    let head = parent.head_commit().unwrap();
    let tree = head.tree().unwrap();
    let entry = tree.find_entry(".gitmodules").unwrap();
    let blob = parent.find_object(entry.oid()).unwrap();

    let cfg =
        gix::config::File::try_from(std::str::from_utf8(&blob.data).expect(".gitmodules is utf8"))
            .expect(".gitmodules must parse as git-config");

    // Both submodules show up with the expected fields.
    let alpha_path = cfg
        .string_by("submodule", Some("alpha".into()), "path")
        .expect("alpha path");
    assert_eq!(alpha_path.as_ref(), "alpha");
    let alpha_url = cfg
        .string_by("submodule", Some("alpha".into()), "url")
        .expect("alpha url");
    assert_eq!(alpha_url.as_ref(), "../modules/alpha.git");

    let email_url = cfg
        .string_by("submodule", Some("user@host.com".into()), "url")
        .expect("email url");
    assert_eq!(email_url.as_ref(), "../modules/user@host.com.git");
}
