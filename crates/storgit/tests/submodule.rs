//! Submodule-layout-specific tests: persistence envelopes
//! ([`Parts`] / [`Snapshot`] / [`ModuleChange`]), the module fetcher,
//! save/load bundling, and the on-disk shape of the parent repo
//! (`.gitmodules`, loose-object budget, deferred parent commits).

mod common;

use std::sync::{Arc, Mutex};

use common::{get_data, mkid, put_data};
use storgit::layout::submodule::{Modules, Parts};
use storgit::{Id, ModuleChange, Store};

fn empty() -> Parts {
    Parts {
        parent: Vec::new(),
        modules: Modules::new(),
        fetcher: None,
    }
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
fn put_roundtrips_through_parts() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"hello");
    parts.apply(store.snapshot().unwrap());
    let reopened = Store::open(parts).unwrap();
    assert_eq!(get_data(&reopened, "alpha").as_deref(), Some(&b"hello"[..]));
}

#[test]
fn history_survives_parts_roundtrip() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"v1");
    put_data(&mut store, "alpha", b"v2");
    parts.apply(store.snapshot().unwrap());
    let reopened = Store::open(parts).unwrap();
    let history = reopened.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..]), Some(&b"v1"[..])]);
}

#[test]
fn snapshot_only_reports_touched_modules() {
    // Writing one entry must not mark any other entry's tarball as dirty.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"1");
    put_data(&mut store, "beta", b"1");
    let _first = store.snapshot().unwrap();
    put_data(&mut store, "alpha", b"2");
    let second = store.snapshot().unwrap();
    assert!(second.parent.is_some(), "parent advances on every put");
    assert!(second.modules.contains_key(&mkid("alpha")));
    assert!(
        !second.modules.contains_key(&mkid("beta")),
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
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"a1");
    put_data(&mut store, "alpha", b"a2");
    put_data(&mut store, "beta", b"b1");
    let bytes = store.save().unwrap();

    let reloaded = Store::load(&bytes).unwrap();
    let mut ids = reloaded.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("alpha"), mkid("beta")]);
    assert_eq!(get_data(&reloaded, "alpha").as_deref(), Some(&b"a2"[..]));
    let history = reloaded.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"a2"[..]), Some(&b"a1"[..])]);
}

#[test]
fn save_is_nondestructive() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"1");
    let _bytes = store.save().unwrap();
    put_data(&mut store, "beta", b"2");
    let mut ids = store.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("alpha"), mkid("beta")]);
}

#[test]
fn save_bundles_every_module_even_ones_not_touched_since_last_snapshot() {
    // snapshot() reports incremental deltas; save() is the full bundle.
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
    assert!(parts.modules.contains_key(&mkid("alpha")));
    store.delete(&mkid("alpha")).unwrap();
    let snap = store.snapshot().unwrap();
    assert!(matches!(
        snap.modules.get(&mkid("alpha")),
        Some(ModuleChange::Deleted)
    ));
    parts.apply(snap);
    assert!(!parts.modules.contains_key(&mkid("alpha")));
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
    const N: usize = 50;
    let mut store = Store::open(empty()).unwrap();
    for i in 0..N {
        store
            .put(
                &mkid(&format!("entry-{i:04}")),
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
    store.archive(&mkid("entry-0000")).unwrap();
    store.delete(&mkid("entry-0001")).unwrap();

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
fn parts_from_first_snapshot_can_be_reopened() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"hi");
    let parts: Parts = store.snapshot().unwrap().into();
    let reopened = Store::open(parts).unwrap();
    assert_eq!(get_data(&reopened, "alpha").as_deref(), Some(&b"hi"[..]));
}

#[test]
fn parent_ref_is_not_updated_between_puts_without_snapshot() {
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
    assert_eq!(ids, vec![mkid("alpha"), mkid("beta")]);
}

#[test]
fn label_cache_survives_parts_roundtrip() {
    let mut parts = empty();
    let mut store = Store::open(empty()).unwrap();
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"data"))
        .unwrap();
    parts.apply(store.snapshot().unwrap());

    let reopened = Store::open(parts).unwrap();
    assert_eq!(reopened.label(&mkid("alpha")), Some(&b"label"[..]));
    assert_eq!(
        reopened.list_labels(),
        vec![(mkid("alpha"), b"label".to_vec())],
    );
}

// -- .gitmodules manifest ----------------------------------------------

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
    let mut store = Store::open(empty()).unwrap();
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let parent_path = tmp.path().join("parent.git");
    let parent = gix::open(&parent_path).unwrap();
    assert!(parent.head_commit().is_err(), "fresh parent has no HEAD");
}

#[test]
fn gitmodules_updates_when_entry_is_archived() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "keep", b"x");
    put_data(&mut store, "drop", b"y");
    store.archive(&mkid("drop")).unwrap();
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

// -- fetcher / lazy loading --------------------------------------------

fn snapshot_backing(entries: &[(&str, &[u8])]) -> (Vec<u8>, Modules) {
    let mut store = Store::open(empty()).unwrap();
    for (name, data) in entries {
        store.put(&mkid(name), Some(b"label"), Some(data)).unwrap();
    }
    let mut parts = empty();
    parts.apply(store.snapshot().unwrap());
    (parts.parent, parts.modules)
}

#[test]
fn fetcher_is_consulted_on_miss_and_result_round_trips() {
    let (parent, modules) = snapshot_backing(&[("alpha", b"hello")]);

    let backing = Arc::new(modules);
    let calls: Arc<Mutex<Vec<Id>>> = Arc::new(Mutex::new(Vec::new()));
    let (backing_for_fetch, calls_for_fetch) = (backing.clone(), calls.clone());

    let parts = Parts {
        parent,
        modules: Modules::new(),
        fetcher: Some(Arc::new(move |id: &Id| {
            calls_for_fetch.lock().unwrap().push(id.clone());
            Ok(backing_for_fetch.get(id).cloned())
        })),
    };
    let store = Store::open(parts).unwrap();
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"hello"[..]));
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        &[mkid("alpha")],
        "fetcher is consulted exactly once for a miss",
    );
}

#[test]
fn fetcher_prewarm_short_circuits_lookup() {
    let (parent, modules) = snapshot_backing(&[("alpha", b"hi")]);
    let calls: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));
    let calls_for_fetch = calls.clone();
    let parts = Parts {
        parent,
        modules,
        fetcher: Some(Arc::new(move |_id: &Id| {
            *calls_for_fetch.lock().unwrap() += 1;
            Ok(None)
        })),
    };
    let store = Store::open(parts).unwrap();
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"hi"[..]));
    assert_eq!(
        *calls.lock().unwrap(),
        0,
        "prewarmed id must not reach the fetcher",
    );
}

#[test]
fn fetcher_ok_none_for_live_id_surfaces_as_error() {
    let (parent, _) = snapshot_backing(&[("alpha", b"hi")]);
    let parts = Parts {
        parent,
        modules: Modules::new(),
        fetcher: Some(Arc::new(|_id: &Id| Ok(None))),
    };
    let store = Store::open(parts).unwrap();
    let err = store
        .get(&mkid("alpha"))
        .expect_err("live id with no backing bytes must error");
    let msg = format!("{err}");
    assert!(
        msg.contains("alpha") && msg.contains("None"),
        "error should name the id and the None-answer cause; got: {msg}",
    );
}

#[test]
fn fetcher_ok_none_for_unknown_id_is_fresh() {
    let parts = Parts {
        parent: Vec::new(),
        modules: Modules::new(),
        fetcher: Some(Arc::new(|_id: &Id| Ok(None))),
    };
    let mut store = Store::open(parts).unwrap();
    store.put(&mkid("fresh"), None, Some(b"v1")).unwrap();
    assert_eq!(get_data(&store, "fresh").as_deref(), Some(&b"v1"[..]));
}

#[test]
fn fetcher_error_propagates_as_error_fetch() {
    let (parent, _) = snapshot_backing(&[("alpha", b"hi")]);
    let parts = Parts {
        parent,
        modules: Modules::new(),
        fetcher: Some(Arc::new(|_id: &Id| Err("db unreachable".into()))),
    };
    let store = Store::open(parts).unwrap();
    let err = store
        .get(&mkid("alpha"))
        .expect_err("fetcher error must propagate");
    let msg = format!("{err}");
    assert!(
        msg.contains("fetch") && msg.contains("db unreachable"),
        "error should carry the fetch source; got: {msg}",
    );
}

#[test]
fn delete_drops_entry_and_history() {
    // Hard-delete erases the submodule's history entirely -- a
    // submodule-only property since subdir can't cheaply rewrite a
    // shared ref.
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "gone", b"bye");
    store.delete(&mkid("gone")).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&mkid("gone")).unwrap().is_none());
    assert!(store.history(&mkid("gone")).unwrap().is_empty());
}

#[test]
fn re_put_after_delete_starts_fresh_history() {
    let mut store = Store::open(empty()).unwrap();
    put_data(&mut store, "alpha", b"v1");
    store.delete(&mkid("alpha")).unwrap();
    put_data(&mut store, "alpha", b"v2");
    let history = store.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..])]);
}

#[test]
fn new_submodule_store_is_usable() {
    let mut store = Store::new().unwrap();
    assert!(store.list().unwrap().is_empty());
    put_data(&mut store, "alpha", b"x");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"x"[..]));
}
