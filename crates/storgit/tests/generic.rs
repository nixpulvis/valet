//! Tests exercising the layout-agnostic [`storgit::Store`] surface.
//! Each test body lives here and is instantiated once per concrete
//! layout via `generic_test!`, which expands into a module named
//! after the test holding two `#[test]` fns: `submodule` and
//! `subdir`. In test output they read as
//! `put_then_get_returns_latest::submodule` / `::subdir`.

mod common;

use common::{get_data, mkid, put_data};
use storgit::{CommitId, Entry, Id, id};

macro_rules! generic_test {
    ($name:ident, |$store:ident| $body:block) => {
        mod $name {
            use super::*;

            #[test]
            fn submodule() {
                #[allow(unused_mut)]
                let mut $store = common::make_submodule_store();
                $body
            }

            #[test]
            fn subdir() {
                #[allow(unused_mut)]
                let mut $store = common::make_subdir_store();
                $body
            }
        }
    };
}

generic_test!(put_then_get_returns_latest, |store| {
    put_data(&mut store, "alpha", b"one");
    put_data(&mut store, "alpha", b"two");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"two"[..]));
});

generic_test!(get_missing_entry_returns_none, |store| {
    assert!(store.get(&mkid("nope")).unwrap().is_none());
});

generic_test!(history_returns_all_versions_newest_first, |store| {
    put_data(&mut store, "alpha", b"v1");
    put_data(&mut store, "alpha", b"v2");
    put_data(&mut store, "alpha", b"v3");
    let history = store.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(
        payloads,
        vec![Some(&b"v3"[..]), Some(&b"v2"[..]), Some(&b"v1"[..])]
    );
});

generic_test!(list_names_live_entries, |store| {
    put_data(&mut store, "a", b"x");
    put_data(&mut store, "b", b"y");
    let mut ids = store.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("a"), mkid("b")]);
});

generic_test!(archive_removes_from_list_but_history_survives, |store| {
    put_data(&mut store, "gone", b"bye");
    store.archive(&mkid("gone")).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&mkid("gone")).unwrap().is_none());
    let history = store.history(&mkid("gone")).unwrap();
    assert_eq!(history.len(), 2, "archive appends a tombstone commit");
    assert_eq!(history[0].data, None, "newest commit is the tombstone");
    assert_eq!(history[0].label, None);
    assert_eq!(history[1].data.as_deref(), Some(&b"bye"[..]));
});

generic_test!(re_put_after_archive_continues_history, |store| {
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
});

generic_test!(
    put_returns_matching_commit_id_for_latest_history_entry,
    |store| {
        let commit: CommitId = store
            .put(&mkid("alpha"), None, Some(b"payload"))
            .unwrap()
            .expect("first put writes a commit");
        let history = store.history(&mkid("alpha")).unwrap();
        assert_eq!(history.first().map(|e| &e.commit), Some(&commit));
    }
);

generic_test!(empty_payload_roundtrips, |store| {
    put_data(&mut store, "alpha", b"");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b""[..]));
});

generic_test!(new_store_is_empty_and_writable, |store| {
    assert!(store.list().unwrap().is_empty());
    put_data(&mut store, "alpha", b"x");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"x"[..]));
});

generic_test!(put_rejects_both_sides_none, |store| {
    assert!(store.put(&mkid("alpha"), None, None).is_err());
});

generic_test!(put_label_and_data_roundtrips, |store| {
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"payload"))
        .unwrap();
    let entry: Entry = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(entry.data.as_deref(), Some(&b"payload"[..]));
});

generic_test!(put_none_slot_carries_prior_blob_forward, |store| {
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
});

generic_test!(put_label_only_is_noop_when_label_matches_prior, |store| {
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
});

generic_test!(put_none_on_fresh_entry_omits_slot, |store| {
    store.put(&mkid("alpha"), Some(b"label"), None).unwrap();
    let entry = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(
        entry.data, None,
        "no prior commit to reuse, so the slot is omitted"
    );
});

generic_test!(put_is_noop_when_tree_matches_head, |store| {
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
});

generic_test!(label_cache_surfaces_via_label_and_list_labels, |store| {
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
});

generic_test!(empty_label_is_not_indexed_but_still_in_history, |store| {
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
});

generic_test!(archive_clears_label_from_cache, |store| {
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"data"))
        .unwrap();
    assert_eq!(store.label(&mkid("alpha")), Some(&b"label"[..]));
    store.archive(&mkid("alpha")).unwrap();
    assert_eq!(store.label(&mkid("alpha")), None);
    assert!(store.list_labels().is_empty());
});

// -- Id validation (layout-independent) --------------------------------

#[test]
fn id_rejects_empty() {
    assert_eq!(Id::new(""), Err(id::Error::Empty));
}

#[test]
fn id_rejects_slash_and_nul() {
    assert_eq!(Id::new("a/b"), Err(id::Error::BadChar('/')));
    assert_eq!(Id::new("a\0b"), Err(id::Error::BadChar('\0')));
}

#[test]
fn id_rejects_quote_and_backslash() {
    assert_eq!(Id::new("a\"b"), Err(id::Error::BadChar('"')));
    assert_eq!(Id::new("a\\b"), Err(id::Error::BadChar('\\')));
}

#[test]
fn id_rejects_control_chars() {
    assert_eq!(Id::new("a\nb"), Err(id::Error::BadChar('\n')));
    assert_eq!(Id::new("a\tb"), Err(id::Error::BadChar('\t')));
    assert_eq!(Id::new("a\rb"), Err(id::Error::BadChar('\r')));
    assert_eq!(Id::new("a\x01b"), Err(id::Error::BadChar('\x01')));
    assert_eq!(Id::new("a\x7fb"), Err(id::Error::BadChar('\x7f')));
}

#[test]
fn id_rejects_leading_dot() {
    assert_eq!(Id::new(".foo"), Err(id::Error::LeadingDot));
    assert_eq!(Id::new("."), Err(id::Error::LeadingDot));
    assert_eq!(Id::new(".."), Err(id::Error::LeadingDot));
}

#[test]
fn id_rejects_git_suffix() {
    assert_eq!(Id::new("foo.git"), Err(id::Error::GitSuffix));
}

#[test]
fn id_rejects_reserved_names() {
    assert_eq!(Id::new("index"), Err(id::Error::Reserved));
}

#[test]
fn id_rejects_too_long() {
    let long = "a".repeat(Id::MAX_LEN + 1);
    assert!(matches!(Id::new(long), Err(id::Error::TooLong { .. })));
}

#[test]
fn id_accepts_reasonable_strings() {
    Id::new("alpha").unwrap();
    Id::new("alpha-beta").unwrap();
    Id::new("user@example.com").unwrap();
    Id::new("01945e9b-3e3f-7b2a-b8ab-8a52c82d4c01").unwrap();
}
