//! Shared test helpers and generic test bodies. Each generic body
//! takes a `Store<L>` so the same assertion runs against every
//! layout; the test files invoke them with the appropriate factory.

#![allow(dead_code)]

use storgit::layout::Layout;
use storgit::layout::submodule::{Parts, SubmoduleLayout};
use storgit::{CommitId, Entry, Id, Store};

pub fn mkid(s: &str) -> Id {
    Id::new(s).unwrap()
}

pub fn put_data<L: Layout>(store: &mut Store<L>, id_str: &str, data: &[u8]) {
    store.put(&mkid(id_str), None, Some(data)).unwrap();
}

pub fn get_data<L: Layout>(store: &Store<L>, id_str: &str) -> Option<Vec<u8>> {
    store.get(&mkid(id_str)).unwrap().and_then(|e| e.data)
}

// -- Factories ---------------------------------------------------------

pub fn make_submodule_store() -> Store<SubmoduleLayout> {
    Store::<SubmoduleLayout>::open(Parts::default()).unwrap()
}

// -- Generic bodies ----------------------------------------------------

pub fn put_then_get_returns_latest<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "alpha", b"one");
    put_data(&mut store, "alpha", b"two");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"two"[..]));
}

pub fn get_missing_entry_returns_none<L: Layout>(store: Store<L>) {
    assert!(store.get(&mkid("nope")).unwrap().is_none());
}

pub fn history_returns_all_versions_newest_first<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "alpha", b"v1");
    put_data(&mut store, "alpha", b"v2");
    put_data(&mut store, "alpha", b"v3");
    let history = store.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(
        payloads,
        vec![Some(&b"v3"[..]), Some(&b"v2"[..]), Some(&b"v1"[..])]
    );
}

pub fn list_names_live_entries<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "a", b"x");
    put_data(&mut store, "b", b"y");
    let mut ids = store.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("a"), mkid("b")]);
}

pub fn archive_removes_from_list_but_history_survives<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "gone", b"bye");
    store.archive(&mkid("gone")).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&mkid("gone")).unwrap().is_none());
    let history = store.history(&mkid("gone")).unwrap();
    assert_eq!(history.len(), 2, "archive appends a tombstone commit");
    assert_eq!(history[0].data, None, "newest commit is the tombstone");
    assert_eq!(history[0].label, None);
    assert_eq!(history[1].data.as_deref(), Some(&b"bye"[..]));
}

pub fn re_put_after_archive_continues_history<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "alpha", b"v1");
    store.archive(&mkid("alpha")).unwrap();
    put_data(&mut store, "alpha", b"v2");
    let history = store.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(
        payloads,
        vec![Some(&b"v2"[..]), None, Some(&b"v1"[..])],
        "history is put -> tombstone -> put, newest first",
    );
}

pub fn delete_drops_entry_and_history<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "gone", b"bye");
    store.delete(&mkid("gone")).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&mkid("gone")).unwrap().is_none());
    assert!(store.history(&mkid("gone")).unwrap().is_empty());
}

pub fn re_put_after_delete_starts_fresh_history<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "alpha", b"v1");
    store.delete(&mkid("alpha")).unwrap();
    put_data(&mut store, "alpha", b"v2");
    let history = store.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..])]);
}

pub fn put_returns_matching_commit_id_for_latest_history_entry<L: Layout>(mut store: Store<L>) {
    let commit: CommitId = store
        .put(&mkid("alpha"), None, Some(b"payload"))
        .unwrap()
        .expect("first put writes a commit");
    let history = store.history(&mkid("alpha")).unwrap();
    assert_eq!(history.first().map(|e| &e.commit), Some(&commit));
}

pub fn empty_payload_roundtrips<L: Layout>(mut store: Store<L>) {
    put_data(&mut store, "alpha", b"");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b""[..]));
}

pub fn new_store_is_empty_and_writable<L: Layout>(mut store: Store<L>) {
    assert!(store.list().unwrap().is_empty());
    put_data(&mut store, "alpha", b"x");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"x"[..]));
}

pub fn put_rejects_both_sides_none<L: Layout>(mut store: Store<L>) {
    assert!(store.put(&mkid("alpha"), None, None).is_err());
}

pub fn put_label_and_data_roundtrips<L: Layout>(mut store: Store<L>) {
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"payload"))
        .unwrap();
    let entry: Entry = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(entry.data.as_deref(), Some(&b"payload"[..]));
}

pub fn put_none_slot_carries_prior_blob_forward<L: Layout>(mut store: Store<L>) {
    store.put(&mkid("alpha"), None, Some(b"payload")).unwrap();
    store.put(&mkid("alpha"), Some(b"label"), None).unwrap();

    let latest = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(
        latest.data.as_deref(),
        Some(&b"payload"[..]),
        "None data reuses the prior commit's data blob"
    );
    assert_eq!(latest.label.as_deref(), Some(&b"label"[..]));

    let history = store.history(&mkid("alpha")).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].data.as_deref(), Some(&b"payload"[..]));
    assert_eq!(history[0].label.as_deref(), Some(&b"label"[..]));
    assert_eq!(history[1].data.as_deref(), Some(&b"payload"[..]));
    assert_eq!(history[1].label, None);
}

pub fn put_label_only_is_noop_when_label_matches_prior<L: Layout>(mut store: Store<L>) {
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"payload"))
        .unwrap();
    assert!(
        store
            .put(&mkid("alpha"), Some(b"label"), None)
            .unwrap()
            .is_none(),
        "label-only put with unchanged label is a no-op (data reused)"
    );
    assert_eq!(store.history(&mkid("alpha")).unwrap().len(), 1);
}

pub fn put_none_on_fresh_entry_omits_slot<L: Layout>(mut store: Store<L>) {
    store.put(&mkid("alpha"), Some(b"label"), None).unwrap();
    let entry = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(
        entry.data, None,
        "no prior commit to reuse, so the slot is omitted"
    );
}

pub fn put_is_noop_when_tree_matches_head<L: Layout>(mut store: Store<L>) {
    assert!(
        store
            .put(&mkid("alpha"), Some(b"m"), Some(b"x"))
            .unwrap()
            .is_some(),
        "first put writes a commit"
    );
    assert!(
        store
            .put(&mkid("alpha"), Some(b"m"), Some(b"x"))
            .unwrap()
            .is_none(),
        "identical put is a no-op"
    );
    assert_eq!(
        store.history(&mkid("alpha")).unwrap().len(),
        1,
        "no second commit was written"
    );
}

pub fn label_cache_surfaces_via_label_and_list_labels<L: Layout>(mut store: Store<L>) {
    store
        .put(&mkid("a"), Some(b"label-a"), Some(b"d1"))
        .unwrap();
    store
        .put(&mkid("b"), Some(b"label-b"), Some(b"d2"))
        .unwrap();
    store.put(&mkid("c"), None, Some(b"d3")).unwrap();

    assert_eq!(store.label(&mkid("a")), Some(&b"label-a"[..]));
    assert_eq!(store.label(&mkid("b")), Some(&b"label-b"[..]));
    assert_eq!(store.label(&mkid("c")), None, "no label set for c");
    assert_eq!(store.label(&mkid("missing")), None);

    let mut listed = store.list_labels();
    listed.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        listed,
        vec![
            (mkid("a"), b"label-a".to_vec()),
            (mkid("b"), b"label-b".to_vec()),
        ],
        "list_labels omits entries whose label is absent",
    );
}

pub fn empty_label_is_not_indexed_but_still_in_history<L: Layout>(mut store: Store<L>) {
    store.put(&mkid("alpha"), Some(b""), Some(b"data")).unwrap();
    assert_eq!(
        store.label(&mkid("alpha")),
        None,
        "empty label is not indexed"
    );
    assert!(store.list_labels().is_empty());

    let entry = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(
        entry.label.as_deref(),
        Some(&b""[..]),
        "empty-bytes label is still recorded in the commit tree",
    );
}

pub fn archive_clears_label_from_cache<L: Layout>(mut store: Store<L>) {
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"data"))
        .unwrap();
    assert_eq!(store.label(&mkid("alpha")), Some(&b"label"[..]));
    store.archive(&mkid("alpha")).unwrap();
    assert_eq!(store.label(&mkid("alpha")), None);
    assert!(store.list_labels().is_empty());
}
