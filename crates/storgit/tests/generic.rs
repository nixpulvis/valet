//! Tests exercising the layout-agnostic [`storgit::Store`] surface.
//! Each test body lives here and is instantiated once per concrete
//! layout via `store_test!`, which expands into a module named
//! after the test holding two `#[test]` fns: `submodule` and
//! `subdir`. In test output they read as
//! `put_then_get_returns_latest::submodule` / `::subdir`.

mod common;

use common::{get_data, mkid, put_data};
use storgit::layout::Layout;
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::SubmoduleLayout;
use storgit::{CommitId, Entry, EntryId, Store, id};

macro_rules! store_test {
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

store_test!(put_then_get_returns_latest, |store| {
    put_data(&mut store, "alpha", b"one");
    put_data(&mut store, "alpha", b"two");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"two"[..]));
});

store_test!(get_missing_entry_returns_none, |store| {
    assert!(store.get(&mkid("nope")).unwrap().is_none());
});

store_test!(history_returns_all_versions_newest_first, |store| {
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

store_test!(list_names_live_entries, |store| {
    put_data(&mut store, "a", b"x");
    put_data(&mut store, "b", b"y");
    let mut ids = store.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("a"), mkid("b")]);
});

store_test!(archive_removes_from_list_but_history_survives, |store| {
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

store_test!(re_put_after_archive_continues_history, |store| {
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

store_test!(
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

store_test!(empty_payload_roundtrips, |store| {
    put_data(&mut store, "alpha", b"");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b""[..]));
});

store_test!(new_store_is_empty_and_writable, |store| {
    assert!(store.list().unwrap().is_empty());
    put_data(&mut store, "alpha", b"x");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"x"[..]));
});

store_test!(put_rejects_both_sides_none, |store| {
    assert!(store.put(&mkid("alpha"), None, None).is_err());
});

store_test!(put_label_and_data_roundtrips, |store| {
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"payload"))
        .unwrap();
    let entry: Entry = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(entry.data.as_deref(), Some(&b"payload"[..]));
});

store_test!(put_none_slot_carries_prior_blob_forward, |store| {
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

store_test!(put_label_only_is_noop_when_label_matches_prior, |store| {
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

store_test!(put_none_on_fresh_entry_omits_slot, |store| {
    store.put(&mkid("alpha"), Some(b"label"), None).unwrap();
    let entry = store.get(&mkid("alpha")).unwrap().expect("live entry");
    assert_eq!(entry.label.as_deref(), Some(&b"label"[..]));
    assert_eq!(
        entry.data, None,
        "no prior commit to reuse, so the slot is omitted"
    );
});

store_test!(put_is_noop_when_tree_matches_head, |store| {
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

store_test!(label_cache_surfaces_via_label_and_list_labels, |store| {
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

store_test!(empty_label_is_not_indexed_but_still_in_history, |store| {
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

store_test!(archive_clears_label_from_cache, |store| {
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"data"))
        .unwrap();
    assert_eq!(store.label(&mkid("alpha")), Some(&b"label"[..]));
    store.archive(&mkid("alpha")).unwrap();
    assert_eq!(store.label(&mkid("alpha")), None);
    assert!(store.list_labels().is_empty());
});

// -- new / open / save / load (layout-agnostic trait methods) ----------
//
// Bodies are parameterised over `L: Layout` and instantiated per layout
// via `directory_test!`. Each body receives a fresh scratch TempDir
// (kept alive for the test's scope) and builds the Store itself, so
// paths can outlive the Store (needed for drop-then-open scenarios).

macro_rules! directory_test {
    ($name:ident, |$tmp:ident| $body:block) => {
        mod $name {
            use super::*;

            fn run<L: Layout>() {
                let $tmp = tempfile::Builder::new()
                    .prefix("storgit-")
                    .tempdir()
                    .unwrap();
                $body
            }

            #[test]
            fn submodule() {
                run::<SubmoduleLayout>();
            }

            #[test]
            fn subdir() {
                run::<SubdirLayout>();
            }
        }
    };
}

directory_test!(new_rejects_existing_path, |tmp| {
    let path = tmp.path().join("repo");
    std::fs::create_dir(&path).unwrap();
    assert!(
        Store::<L>::new(path).is_err(),
        "new must refuse a path that already exists",
    );
});

directory_test!(open_reopens_existing_store, |tmp| {
    let path = tmp.path().join("repo");
    {
        let mut store = Store::<L>::new(path.clone()).unwrap();
        put_data(&mut store, "alpha", b"v1");
        put_data(&mut store, "alpha", b"v2");
        put_data(&mut store, "beta", b"b1");
    }
    let reopened = Store::<L>::open(path).unwrap();
    let mut ids = reopened.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("alpha"), mkid("beta")]);
    assert_eq!(get_data(&reopened, "alpha").as_deref(), Some(&b"v2"[..]));
    let history = reopened.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..]), Some(&b"v1"[..])]);
});

directory_test!(open_reopens_label_cache, |tmp| {
    let path = tmp.path().join("repo");
    {
        let mut store = Store::<L>::new(path.clone()).unwrap();
        store
            .put(&mkid("alpha"), Some(b"label-a"), Some(b"data-a"))
            .unwrap();
        store
            .put(&mkid("beta"), Some(b"label-b"), Some(b"data-b"))
            .unwrap();
    }
    let reopened = Store::<L>::open(path).unwrap();
    assert_eq!(reopened.label(&mkid("alpha")), Some(&b"label-a"[..]));
    let mut labels = reopened.list_labels();
    labels.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        labels,
        vec![
            (mkid("alpha"), b"label-a".to_vec()),
            (mkid("beta"), b"label-b".to_vec()),
        ],
    );
});

directory_test!(save_load_roundtrips_all_state, |tmp| {
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    let bytes = {
        let mut store = Store::<L>::new(src).unwrap();
        put_data(&mut store, "alpha", b"a1");
        put_data(&mut store, "alpha", b"a2");
        put_data(&mut store, "beta", b"b1");
        store.save().unwrap()
    };
    let reloaded = Store::<L>::load(&bytes, dst).unwrap();
    let mut ids = reloaded.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("alpha"), mkid("beta")]);
    assert_eq!(get_data(&reloaded, "alpha").as_deref(), Some(&b"a2"[..]));
    let history = reloaded.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"a2"[..]), Some(&b"a1"[..])]);
});

directory_test!(save_is_nondestructive, |tmp| {
    let path = tmp.path().join("repo");
    let mut store = Store::<L>::new(path).unwrap();
    put_data(&mut store, "alpha", b"1");
    let _bytes = store.save().unwrap();
    put_data(&mut store, "beta", b"2");
    let mut ids = store.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("alpha"), mkid("beta")]);
});

directory_test!(load_rejects_nonempty_target, |tmp| {
    let src = tmp.path().join("src");
    let dst = tmp.path().join("dst");
    let bytes = {
        let mut store = Store::<L>::new(src).unwrap();
        put_data(&mut store, "alpha", b"x");
        store.save().unwrap()
    };
    std::fs::create_dir(&dst).unwrap();
    std::fs::write(dst.join("stray"), b"existing").unwrap();
    assert!(
        Store::<L>::load(&bytes, dst).is_err(),
        "load must refuse a non-empty target path",
    );
});

// -- EntryId validation (layout-independent) --------------------------------

#[test]
fn id_rejects_empty() {
    assert_eq!(EntryId::new(""), Err(id::EntryIdError::Empty));
}

#[test]
fn id_rejects_slash_and_nul() {
    assert_eq!(EntryId::new("a/b"), Err(id::EntryIdError::BadChar('/')));
    assert_eq!(EntryId::new("a\0b"), Err(id::EntryIdError::BadChar('\0')));
}

#[test]
fn id_rejects_quote_and_backslash() {
    assert_eq!(EntryId::new("a\"b"), Err(id::EntryIdError::BadChar('"')));
    assert_eq!(EntryId::new("a\\b"), Err(id::EntryIdError::BadChar('\\')));
}

#[test]
fn id_rejects_control_chars() {
    assert_eq!(EntryId::new("a\nb"), Err(id::EntryIdError::BadChar('\n')));
    assert_eq!(EntryId::new("a\tb"), Err(id::EntryIdError::BadChar('\t')));
    assert_eq!(EntryId::new("a\rb"), Err(id::EntryIdError::BadChar('\r')));
    assert_eq!(
        EntryId::new("a\x01b"),
        Err(id::EntryIdError::BadChar('\x01'))
    );
    assert_eq!(
        EntryId::new("a\x7fb"),
        Err(id::EntryIdError::BadChar('\x7f'))
    );
}

#[test]
fn id_rejects_leading_dot() {
    assert_eq!(EntryId::new(".foo"), Err(id::EntryIdError::LeadingDot));
    assert_eq!(EntryId::new("."), Err(id::EntryIdError::LeadingDot));
    assert_eq!(EntryId::new(".."), Err(id::EntryIdError::LeadingDot));
}

#[test]
fn id_rejects_git_suffix() {
    assert_eq!(EntryId::new("foo.git"), Err(id::EntryIdError::GitSuffix));
}

#[test]
fn id_rejects_reserved_names() {
    assert_eq!(EntryId::new("index"), Err(id::EntryIdError::Reserved));
}

#[test]
fn id_rejects_too_long() {
    let long = "a".repeat(EntryId::MAX_LEN + 1);
    assert!(matches!(
        EntryId::new(long),
        Err(id::EntryIdError::TooLong { .. })
    ));
}

#[test]
fn id_accepts_reasonable_strings() {
    EntryId::new("alpha").unwrap();
    EntryId::new("alpha-beta").unwrap();
    EntryId::new("user@example.com").unwrap();
    EntryId::new("01945e9b-3e3f-7b2a-b8ab-8a52c82d4c01").unwrap();
}

store_test!(remotes_starts_empty, |store| {
    assert!(store.remotes().unwrap().is_empty());
});

store_test!(add_remote_then_list, |store| {
    store
        .add_remote("origin", "https://example.com/repo.git")
        .unwrap();
    let rs = store.remotes().unwrap();
    assert_eq!(rs.len(), 1);
    assert_eq!(rs[0].0, "origin");
    assert_eq!(rs[0].1, "https://example.com/repo.git");
});

store_test!(remove_remote_clears_it, |store| {
    store.add_remote("origin", "url").unwrap();
    store.remove_remote("origin").unwrap();
    assert!(store.remotes().unwrap().is_empty());
});

store_test!(remove_unknown_remote_errors, |store| {
    assert!(store.remove_remote("missing").is_err());
});

store_test!(add_duplicate_remote_errors, |store| {
    store.add_remote("origin", "url1").unwrap();
    assert!(store.add_remote("origin", "url2").is_err());
});

store_test!(fetch_unknown_remote_errors, |store| {
    assert!(store.fetch("nope").is_err());
});

store_test!(push_unknown_remote_errors, |store| {
    assert!(store.push("nope").is_err());
});

store_test!(push_returns_unsupported_when_remote_exists, |store| {
    store.add_remote("origin", "file:///tmp/nope").unwrap();
    let err = store.push("origin").unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("not yet supported") || msg.contains("rejected"),
        "expected unsupported/rejected message, got: {msg}"
    );
});

store_test!(fetch_from_local_repo_lands_refs, |store| {
    // Populate a second store of the same layout; fetch from its
    // git_dir via file:// URL. Destination HEAD must not move;
    // remote-tracking refs must land.
    let mut src = common::make_store_like(&store);
    put_data(&mut src, "alpha", b"hello");
    // For submodule, puts buffer until snapshot/save flushes the
    // parent ref; call snapshot to persist the parent HEAD.
    src.flush_for_test();

    let url = format!("file://{}", src.git_dir().display());
    store.add_remote("src", &url).unwrap();

    let head_before = read_head_ref(&store.git_dir());
    store.fetch("src").unwrap();
    let head_after = read_head_ref(&store.git_dir());
    assert_eq!(head_before, head_after, "fetch must not move local HEAD");

    let loose = store.git_dir().join("refs/remotes/src/main");
    let packed = store.git_dir().join("packed-refs");
    assert!(
        loose.exists() || packed.exists(),
        "expected refs/remotes/src/main (loose or packed) after fetch"
    );
});

fn read_head_ref(git_dir: &std::path::Path) -> Option<String> {
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(ref_path) = head.strip_prefix("ref: ") {
        std::fs::read_to_string(git_dir.join(ref_path))
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        Some(head.to_string())
    }
}
