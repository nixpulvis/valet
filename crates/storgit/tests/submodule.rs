//! Submodule-layout-specific tests: persistence envelopes
//! ([`Parts`] / [`Snapshot`] / [`ModuleChange`]), the module fetcher,
//! save/load bundling, and the on-disk shape of the parent repo
//! (`.gitmodules`, loose-object budget, deferred parent commits).

mod common;

use std::sync::{Arc, Mutex};

use common::{
    count_loose_objects, dir_size, extract_to_tmp, fresh_submodule as fresh, get_data,
    load_submodule_bytes as load_bytes, mkid, open_with_parts as open_with, put_data,
};
use storgit::layout::submodule::{ModuleChange, ModuleFetcher, Modules, Parts};
use storgit::{EntryId, Store, SubmoduleLayout};
use tempfile::TempDir;

fn empty() -> Parts {
    Parts {
        parent: Vec::new(),
        modules: Modules::new(),
    }
}

/// Shared fetcher state for fetcher tests. Holds the module bytes
/// the fetcher will serve from and a log of every id it was asked
/// for. Build one with `BackingStore::new`, hand its fetcher to the
/// store via [`open_with_fetcher`], then [`insert`]/[`extend`] as
/// the test needs and read [`calls`] to assert on fetch behaviour.
#[derive(Clone)]
struct BackingStore {
    modules: Arc<Mutex<Modules>>,
    calls: Arc<Mutex<Vec<EntryId>>>,
}

impl BackingStore {
    fn new() -> Self {
        Self {
            modules: Arc::new(Mutex::new(Modules::new())),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn extend(&self, modules: Modules) {
        self.modules.lock().unwrap().extend(modules);
    }

    fn calls(&self) -> Vec<EntryId> {
        self.calls.lock().unwrap().clone()
    }

    fn fetcher(&self) -> ModuleFetcher {
        let modules = self.modules.clone();
        let calls = self.calls.clone();
        Arc::new(move |id: &EntryId| {
            calls.lock().unwrap().push(id.clone());
            Ok(modules.lock().unwrap().get(id).cloned())
        })
    }
}

/// Like [`open_with`] but also installs a fetcher that serves from
/// a fresh [`BackingStore`], returned so the test can populate it
/// and inspect call history.
fn open_with_fetcher(parts: Parts) -> (TempDir, Store<SubmoduleLayout>, BackingStore) {
    let backing = BackingStore::new();
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::new(path)
        .unwrap()
        .with_parts(parts)
        .unwrap()
        .with_fetcher(backing.fetcher());
    (scratch, store, backing)
}

#[test]
fn open_empty_roundtrips() {
    let (_tmp, mut store) = fresh();
    let snap = store.snapshot().expect("snapshot");
    assert!(snap.parent.is_some(), "fresh store must publish its parent");
    assert!(snap.modules.is_empty());
    let mut parts = empty();
    parts.apply(snap);
    let (_tmp2, _reopened) = open_with(parts);
}

#[test]
fn put_roundtrips_through_parts() {
    let mut parts = empty();
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"hello");
    parts.apply(store.snapshot().unwrap());
    let (_tmp2, reopened) = open_with(parts);
    assert_eq!(get_data(&reopened, "alpha").as_deref(), Some(&b"hello"[..]));
}

#[test]
fn history_survives_parts_roundtrip() {
    let mut parts = empty();
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"v1");
    put_data(&mut store, "alpha", b"v2");
    parts.apply(store.snapshot().unwrap());
    let (_tmp2, reopened) = open_with(parts);
    let history = reopened.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..]), Some(&b"v1"[..])]);
}

#[test]
fn snapshot_only_reports_touched_modules() {
    // Writing one entry must not mark any other entry's tarball as dirty.
    let (_tmp, mut store) = fresh();
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
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"1");
    let _first = store.snapshot().unwrap();
    let second = store.snapshot().unwrap();
    assert!(second.parent.is_none());
    assert!(second.modules.is_empty());
}

#[test]
fn save_load_roundtrips_all_state() {
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"a1");
    put_data(&mut store, "alpha", b"a2");
    put_data(&mut store, "beta", b"b1");
    let bytes = store.save().unwrap();

    let (_tmp2, reloaded) = load_bytes(&bytes);
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
    let (_tmp, mut store) = fresh();
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
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"a");
    put_data(&mut store, "beta", b"b");
    let _drain_dirty = store.snapshot().unwrap();
    let bytes = store.save().unwrap();

    let (_tmp2, reloaded) = load_bytes(&bytes);
    assert_eq!(get_data(&reloaded, "alpha").as_deref(), Some(&b"a"[..]));
    assert_eq!(get_data(&reloaded, "beta").as_deref(), Some(&b"b"[..]));
}

#[test]
fn delete_emits_module_deletion_in_snapshot() {
    let mut parts = empty();
    let (_tmp, mut store) = fresh();
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
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"payload");
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let module = tmp.path().join("modules").join("alpha.git");
    let size = dir_size(&module).unwrap();
    assert!(
        size < 1024,
        "fresh submodule should be <1 KB on disk; got {size} B. \
         likely cause: init_bare templates (hooks/, info/, description) \
         are being shipped again."
    );
}

#[test]
fn parent_objects_stay_bounded_after_many_puts() {
    const N: usize = 50;
    let (_tmp, mut store) = fresh();
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

    let tmp = extract_to_tmp(&bytes);
    let parent_objects = tmp.path().join("parent.git").join("objects");
    let loose = count_loose_objects(&parent_objects);
    assert!(
        loose <= 5,
        "parent.git has {loose} loose objects after {N} puts; \
         expected ~3 (parent commit + tree + .gitmodules)",
    );
}

#[test]
fn parent_collapses_dirty_run_into_one_flush_commit() {
    // Many dirty operations between flushes should collapse into a
    // single parent commit, so the chain doesn't grow per-put.
    let (_tmp, mut store) = fresh();
    for i in 0..50 {
        put_data(&mut store, &format!("entry-{i:04}"), b"x");
    }
    store.archive(&mkid("entry-0000")).unwrap();
    store.delete(&mkid("entry-0001")).unwrap();

    let bytes = store.save().unwrap();
    let tmp = extract_to_tmp(&bytes);
    let parent = gix::open(tmp.path().join("parent.git")).unwrap();
    let head = parent.head_commit().unwrap();
    let count = head.ancestors().all().unwrap().count();
    assert_eq!(count, 1, "all dirty ops were folded into one flush");
}

#[test]
fn parent_chain_grows_with_each_flush() {
    // Successive flushes chain so merge_base lookups can find the
    // common ancestor during sync.
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"v1");
    store.snapshot().unwrap();
    put_data(&mut store, "beta", b"v1");
    store.snapshot().unwrap();
    put_data(&mut store, "gamma", b"v1");
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let parent = gix::open(tmp.path().join("parent.git")).unwrap();
    let head = parent.head_commit().unwrap();
    let count = head.ancestors().all().unwrap().count();
    assert_eq!(count, 3, "one commit per flush");
}

#[test]
fn parts_from_first_snapshot_can_be_reopened() {
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"hi");
    let parts: Parts = store.snapshot().unwrap().into();
    let (_tmp2, reopened) = open_with(parts);
    assert_eq!(get_data(&reopened, "alpha").as_deref(), Some(&b"hi"[..]));
}

#[test]
fn parent_ref_is_not_updated_between_puts_without_snapshot() {
    let (_tmp, mut store) = fresh();
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
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"1");
    parts.apply(store.snapshot().unwrap());
    put_data(&mut store, "beta", b"2");
    parts.apply(store.snapshot().unwrap());
    let (_tmp2, reopened) = open_with(parts);
    let mut ids = reopened.list().unwrap();
    ids.sort();
    assert_eq!(ids, vec![mkid("alpha"), mkid("beta")]);
}

#[test]
fn label_cache_survives_parts_roundtrip() {
    let mut parts = empty();
    let (_tmp, mut store) = fresh();
    store
        .put(&mkid("alpha"), Some(b"label"), Some(b"data"))
        .unwrap();
    parts.apply(store.snapshot().unwrap());

    let (_tmp2, reopened) = open_with(parts);
    assert_eq!(reopened.label(&mkid("alpha")), Some(&b"label"[..]));
    assert_eq!(
        reopened.list_labels(),
        vec![(mkid("alpha"), b"label".to_vec())],
    );
}

// -- .gitmodules manifest ----------------------------------------------

#[test]
fn gitmodules_blob_appears_in_parent_tree_after_put() {
    let (_tmp, mut store) = fresh();
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
    let (_tmp, mut store) = fresh();
    let bytes = store.save().unwrap();

    let tmp = extract_to_tmp(&bytes);
    let parent_path = tmp.path().join("parent.git");
    let parent = gix::open(&parent_path).unwrap();
    assert!(parent.head_commit().is_err(), "fresh parent has no HEAD");
}

#[test]
fn gitmodules_updates_when_entry_is_archived() {
    let (_tmp, mut store) = fresh();
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
    let (_tmp, mut store) = fresh();
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
    let (_tmp, mut store) = fresh();
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
    let parts = Parts {
        parent,
        modules: Modules::new(),
    };
    let (_tmp, store, backing) = open_with_fetcher(parts);
    backing.extend(modules);
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"hello"[..]));
    assert_eq!(
        backing.calls(),
        vec![mkid("alpha")],
        "fetcher is consulted exactly once for a miss",
    );
}

#[test]
fn fetcher_prewarm_short_circuits_lookup() {
    let (parent, modules) = snapshot_backing(&[("alpha", b"hi")]);
    let parts = Parts { parent, modules };
    let (_tmp, store, backing) = open_with_fetcher(parts);
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"hi"[..]));
    assert!(
        backing.calls().is_empty(),
        "prewarmed id must not reach the fetcher",
    );
}

#[test]
fn fetcher_ok_none_for_live_id_surfaces_as_error() {
    let (parent, _) = snapshot_backing(&[("alpha", b"hi")]);
    let parts = Parts {
        parent,
        modules: Modules::new(),
    };
    let (_tmp, store, _backing) = open_with_fetcher(parts);
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
    let (_tmp, mut store, _backing) = open_with_fetcher(empty());
    store.put(&mkid("fresh"), None, Some(b"v1")).unwrap();
    assert_eq!(get_data(&store, "fresh").as_deref(), Some(&b"v1"[..]));
}

#[test]
fn fetcher_error_propagates_as_error_fetch() {
    let (parent, _) = snapshot_backing(&[("alpha", b"hi")]);
    let parts = Parts {
        parent,
        modules: Modules::new(),
    };
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let fetcher: ModuleFetcher = Arc::new(|_id: &EntryId| Err("db unreachable".into()));
    let store = Store::<SubmoduleLayout>::new(path)
        .unwrap()
        .with_parts(parts)
        .unwrap()
        .with_fetcher(fetcher);
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
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "gone", b"bye");
    store.delete(&mkid("gone")).unwrap();
    assert!(store.list().unwrap().is_empty());
    assert!(store.get(&mkid("gone")).unwrap().is_none());
    assert!(store.history(&mkid("gone")).unwrap().is_empty());
}

#[test]
fn re_put_after_delete_starts_fresh_history() {
    let (_tmp, mut store) = fresh();
    put_data(&mut store, "alpha", b"v1");
    store.delete(&mkid("alpha")).unwrap();
    put_data(&mut store, "alpha", b"v2");
    let history = store.history(&mkid("alpha")).unwrap();
    let payloads: Vec<Option<&[u8]>> = history.iter().map(|e| e.data.as_deref()).collect();
    assert_eq!(payloads, vec![Some(&b"v2"[..])]);
}

#[test]
fn new_submodule_store_is_usable() {
    let (_tmp, mut store) = fresh();
    assert!(store.list().unwrap().is_empty());
    put_data(&mut store, "alpha", b"x");
    assert_eq!(get_data(&store, "alpha").as_deref(), Some(&b"x"[..]));
}

// --- Merge tests (submodule layout) ----------------------------

use storgit::merge::{ApplyMode, MergeStatus, Side};

fn pull_url_sub(store: &Store<SubmoduleLayout>) -> String {
    format!("file://{}", store.git_dir().display())
}

fn flush_sub(store: &mut Store<SubmoduleLayout>) {
    // snapshot returns a delta; the side effect is that the parent
    // ref is materialised on disk so a remote can fetch from it.
    store.snapshot().unwrap();
}

#[test]
fn pull_no_op_on_identical_state_sub() {
    let (_a_tmp, mut a) = fresh();
    let (_b_tmp, b) = fresh();
    a.add_remote("b", &pull_url_sub(&b)).unwrap();
    let status = a.pull("b").unwrap();
    match status {
        MergeStatus::Clean(advanced) => assert!(advanced.is_empty()),
        _ => panic!("expected clean no-op"),
    }
}

#[test]
fn pull_loads_into_empty_store_sub() {
    let (_src_tmp, mut src) = fresh();
    put_data(&mut src, "alpha", b"hi");
    flush_sub(&mut src);
    let (_dst_tmp, mut dst) = fresh();
    dst.add_remote("src", &pull_url_sub(&src)).unwrap();
    let status = dst.pull("src").unwrap();
    match status {
        MergeStatus::Clean(advanced) => {
            assert_eq!(advanced.len(), 1);
            assert!(advanced.contains_key(&mkid("alpha")));
        }
        _ => panic!("expected clean"),
    }
    assert_eq!(get_data(&dst, "alpha").as_deref(), Some(&b"hi"[..]));
}

#[test]
fn pull_fast_forwards_when_local_is_ancestor_sub() {
    let (_src_tmp, mut src) = fresh();
    put_data(&mut src, "alpha", b"v1");
    flush_sub(&mut src);
    let (_dst_tmp, mut dst) = fresh();
    dst.add_remote("src", &pull_url_sub(&src)).unwrap();
    dst.pull("src").unwrap();

    put_data(&mut src, "alpha", b"v2");
    flush_sub(&mut src);
    let status = dst.pull("src").unwrap();
    match status {
        MergeStatus::Clean(advanced) => assert_eq!(advanced.len(), 1),
        _ => panic!("expected ff"),
    }
    assert_eq!(get_data(&dst, "alpha").as_deref(), Some(&b"v2"[..]));
}

#[test]
fn pull_clean_3way_disjoint_ids_sub() {
    let (_a_tmp, mut a) = fresh();
    let (_b_tmp, mut b) = fresh();
    put_data(&mut a, "alpha", b"a");
    flush_sub(&mut a);
    put_data(&mut b, "beta", b"b");
    flush_sub(&mut b);

    a.add_remote("b", &pull_url_sub(&b)).unwrap();
    let status = a.pull("b").unwrap();
    match status {
        MergeStatus::Clean(advanced) => {
            assert!(advanced.contains_key(&mkid("beta")));
        }
        _ => panic!("expected clean for disjoint ids"),
    }
    assert_eq!(get_data(&a, "alpha").as_deref(), Some(&b"a"[..]));
    assert_eq!(get_data(&a, "beta").as_deref(), Some(&b"b"[..]));
}

#[test]
fn pull_conflict_then_resolve_local_sub() {
    let (_a_tmp, mut a) = fresh();
    let (_b_tmp, mut b) = fresh();
    put_data(&mut a, "alpha", b"shared");
    flush_sub(&mut a);
    a.add_remote("b", &pull_url_sub(&b)).unwrap();
    b.add_remote("a", &pull_url_sub(&a)).unwrap();
    b.pull("a").unwrap();

    put_data(&mut a, "alpha", b"a-version");
    flush_sub(&mut a);
    put_data(&mut b, "alpha", b"b-version");
    flush_sub(&mut b);

    let status = a.pull("b").unwrap();
    let mut progress = match status {
        MergeStatus::Conflicted(p) => p,
        _ => panic!("expected conflict"),
    };
    assert!(a.merge_in_progress());
    assert_eq!(progress.conflicts().len(), 1);
    assert_eq!(progress.conflicts()[0].id.as_str(), "alpha");

    progress.pick(mkid("alpha"), Side::Local).unwrap();
    let resolution = progress.resolve().unwrap();
    a.merge(resolution).unwrap();
    assert!(!a.merge_in_progress());
    assert_eq!(get_data(&a, "alpha").as_deref(), Some(&b"a-version"[..]));
}

#[test]
fn pull_conflict_then_resolve_incoming_sub() {
    let (_a_tmp, mut a) = fresh();
    let (_b_tmp, mut b) = fresh();
    put_data(&mut a, "alpha", b"shared");
    flush_sub(&mut a);
    a.add_remote("b", &pull_url_sub(&b)).unwrap();
    b.add_remote("a", &pull_url_sub(&a)).unwrap();
    b.pull("a").unwrap();

    put_data(&mut a, "alpha", b"a-version");
    flush_sub(&mut a);
    put_data(&mut b, "alpha", b"b-version");
    flush_sub(&mut b);

    let status = a.pull("b").unwrap();
    let mut progress = match status {
        MergeStatus::Conflicted(p) => p,
        _ => panic!("expected conflict"),
    };
    progress.pick(mkid("alpha"), Side::Incoming).unwrap();
    let resolution = progress.resolve().unwrap();
    a.merge(resolution).unwrap();
    assert_eq!(get_data(&a, "alpha").as_deref(), Some(&b"b-version"[..]));
}

#[test]
fn put_during_merge_errors_sub() {
    let (_a_tmp, mut a) = fresh();
    let (_b_tmp, mut b) = fresh();
    put_data(&mut a, "alpha", b"shared");
    flush_sub(&mut a);
    a.add_remote("b", &pull_url_sub(&b)).unwrap();
    b.add_remote("a", &pull_url_sub(&a)).unwrap();
    b.pull("a").unwrap();
    put_data(&mut a, "alpha", b"a-v");
    flush_sub(&mut a);
    put_data(&mut b, "alpha", b"b-v");
    flush_sub(&mut b);
    let status = a.pull("b").unwrap();
    assert!(matches!(status, MergeStatus::Conflicted(_)));
    assert!(a.put(&mkid("alpha"), None, Some(b"x")).is_err());
}

#[test]
fn apply_into_empty_loads_state() {
    // a's first snapshot is its full state (Parts.apply folds it).
    let (_a_tmp, mut a) = fresh();
    put_data(&mut a, "alpha", b"hello");
    let snap = a.snapshot().unwrap();
    let mut parts = Parts::default();
    parts.apply(snap);

    let (_b_tmp, mut b) = fresh();
    let status = b.apply(parts).unwrap();
    match status {
        MergeStatus::Clean(advanced) => assert!(advanced.contains_key(&mkid("alpha"))),
        _ => panic!("expected clean load"),
    }
    assert_eq!(get_data(&b, "alpha").as_deref(), Some(&b"hello"[..]));
}

#[test]
fn apply_onto_populated_clean_merge() {
    let (_a_tmp, mut a) = fresh();
    put_data(&mut a, "alpha", b"a");
    let snap_a = a.snapshot().unwrap();
    let mut parts_a = Parts::default();
    parts_a.apply(snap_a);

    let (_b_tmp, mut b) = fresh();
    put_data(&mut b, "beta", b"b");
    flush_sub(&mut b);

    let status = b.apply(parts_a).unwrap();
    match status {
        MergeStatus::Clean(advanced) => {
            assert!(advanced.contains_key(&mkid("alpha")));
        }
        _ => panic!("expected clean disjoint-id merge"),
    }
    assert_eq!(get_data(&b, "alpha").as_deref(), Some(&b"a"[..]));
    assert_eq!(get_data(&b, "beta").as_deref(), Some(&b"b"[..]));
}

#[test]
fn apply_onto_populated_with_conflict() {
    // Build a shared starting state via Parts.
    let (_seed_tmp, mut seed) = fresh();
    put_data(&mut seed, "alpha", b"shared");
    let snap_shared = seed.snapshot().unwrap();
    let mut parts_shared = Parts::default();
    parts_shared.apply(snap_shared);

    let (_a_tmp, mut a) = fresh();
    a.apply(parts_shared.clone()).unwrap();
    let (_b_tmp, mut b) = fresh();
    b.apply(parts_shared).unwrap();

    // Diverge.
    put_data(&mut a, "alpha", b"a-version");
    let snap_a_delta = a.snapshot().unwrap();
    let mut parts_a_delta = Parts::default();
    parts_a_delta.apply(snap_a_delta);
    put_data(&mut b, "alpha", b"b-version");

    let status = b.apply(parts_a_delta).unwrap();
    let mut progress = match status {
        MergeStatus::Conflicted(p) => p,
        _ => panic!("expected conflict from divergent puts"),
    };
    assert_eq!(progress.conflicts().len(), 1);
    assert_eq!(progress.conflicts()[0].id.as_str(), "alpha");
    progress.pick(mkid("alpha"), Side::Local).unwrap();
    let resolution = progress.resolve().unwrap();
    b.merge(resolution).unwrap();
    assert_eq!(get_data(&b, "alpha").as_deref(), Some(&b"b-version"[..]));
}

#[test]
fn apply_ff_only_accepts_fast_forward() {
    // Build a's first state, hand to b via apply.
    let (_a_tmp, mut a) = fresh();
    put_data(&mut a, "alpha", b"v1");
    let snap_v1 = a.snapshot().unwrap();
    let mut parts_v1 = Parts::default();
    parts_v1.apply(snap_v1);

    let (_b_tmp, mut b) = fresh();
    b.apply(parts_v1).unwrap();

    // Advance a; the next snapshot is a fast-forward delta on top
    // of the state b already has.
    put_data(&mut a, "alpha", b"v2");
    let snap_v2 = a.snapshot().unwrap();
    let mut parts_v2 = Parts::default();
    parts_v2.apply(snap_v2);

    let status = b.apply_with(parts_v2, ApplyMode::FastForwardOnly).unwrap();
    match status {
        MergeStatus::Clean(advanced) => {
            assert!(advanced.contains_key(&mkid("alpha")));
        }
        _ => panic!("expected clean ff"),
    }
    assert_eq!(get_data(&b, "alpha").as_deref(), Some(&b"v2"[..]));
}

#[test]
fn apply_ff_only_accepts_new_id() {
    // b sends a wholly-new id to a. New ids are ff-equivalent.
    let (_a_tmp, mut a) = fresh();
    put_data(&mut a, "alpha", b"a");
    let snap_a = a.snapshot().unwrap();
    let mut parts_a = Parts::default();
    parts_a.apply(snap_a);
    let (_b_tmp, mut b) = fresh();
    b.apply(parts_a).unwrap();

    put_data(&mut b, "beta", b"b");
    let snap_b = b.snapshot().unwrap();
    let mut parts_b = Parts::default();
    parts_b.apply(snap_b);

    let status = a.apply_with(parts_b, ApplyMode::FastForwardOnly).unwrap();
    match status {
        MergeStatus::Clean(advanced) => {
            assert!(advanced.contains_key(&mkid("beta")));
        }
        _ => panic!("expected clean for new-id ff"),
    }
}

#[test]
fn apply_ff_only_rejects_divergent() {
    // Both sides write to alpha from a shared base -> divergent.
    let (_seed_tmp, mut seed) = fresh();
    put_data(&mut seed, "alpha", b"shared");
    let snap_shared = seed.snapshot().unwrap();
    let mut parts_shared = Parts::default();
    parts_shared.apply(snap_shared);

    let (_a_tmp, mut a) = fresh();
    a.apply(parts_shared.clone()).unwrap();
    let (_b_tmp, mut b) = fresh();
    b.apply(parts_shared).unwrap();

    put_data(&mut a, "alpha", b"a-v");
    let snap_a_delta = a.snapshot().unwrap();
    let mut parts_a_delta = Parts::default();
    parts_a_delta.apply(snap_a_delta);
    put_data(&mut b, "alpha", b"b-v");

    let result = b.apply_with(parts_a_delta, ApplyMode::FastForwardOnly);
    let err = match result {
        Err(e) => e,
        Ok(_) => panic!("expected NotFastForward error"),
    };
    let msg = format!("{err}");
    assert!(
        msg.contains("non-fast-forward") && msg.contains("alpha"),
        "expected NotFastForward listing alpha, got: {msg}"
    );
    // Local state must be untouched: alpha still at b-v.
    assert_eq!(get_data(&b, "alpha").as_deref(), Some(&b"b-v"[..]));
}

#[test]
fn apply_ff_only_no_op_on_identical() {
    let (_a_tmp, mut a) = fresh();
    put_data(&mut a, "alpha", b"x");
    let snap_a = a.snapshot().unwrap();
    let mut parts_a = Parts::default();
    parts_a.apply(snap_a);
    let (_b_tmp, mut b) = fresh();
    b.apply(parts_a.clone()).unwrap();

    let status = b.apply_with(parts_a, ApplyMode::FastForwardOnly).unwrap();
    match status {
        MergeStatus::Clean(advanced) => assert!(advanced.is_empty()),
        _ => panic!("expected clean no-op"),
    }
}

#[test]
fn abort_clears_merge_state_sub() {
    let (_a_tmp, mut a) = fresh();
    let (_b_tmp, mut b) = fresh();
    put_data(&mut a, "alpha", b"shared");
    flush_sub(&mut a);
    a.add_remote("b", &pull_url_sub(&b)).unwrap();
    b.add_remote("a", &pull_url_sub(&a)).unwrap();
    b.pull("a").unwrap();
    put_data(&mut a, "alpha", b"a-v");
    flush_sub(&mut a);
    put_data(&mut b, "alpha", b"b-v");
    flush_sub(&mut b);
    let status = a.pull("b").unwrap();
    assert!(matches!(status, MergeStatus::Conflicted(_)));
    assert!(a.merge_in_progress());
    a.abort().unwrap();
    assert!(!a.merge_in_progress());
    a.put(&mkid("alpha"), None, Some(b"new")).unwrap();
}

#[test]
fn bidirectional_pull_converges_after_archive_delete_and_add() {
    // Mirrors examples/sync.rs: a shared seed, divergent edits on A
    // and B (including archive on A and delete on B), then pull both
    // ways. Both sides must converge to the same state.
    let (_seed_tmp, mut seed) = fresh();
    put_data(&mut seed, "alpha", b"a1");
    put_data(&mut seed, "beta", b"b1");
    put_data(&mut seed, "gamma", b"g1");
    flush_sub(&mut seed);
    let blob = seed.save().unwrap();

    let a_scratch = tempfile::tempdir().unwrap();
    let b_scratch = tempfile::tempdir().unwrap();
    let mut a = Store::<SubmoduleLayout>::load(&blob, a_scratch.path().join("a")).unwrap();
    let mut b = Store::<SubmoduleLayout>::load(&blob, b_scratch.path().join("b")).unwrap();

    a.put(&mkid("alpha"), None, Some(b"a2-from-A")).unwrap();
    a.archive(&mkid("beta")).unwrap();
    a.put(&mkid("delta"), None, Some(b"d1")).unwrap();
    flush_sub(&mut a);

    b.put(&mkid("alpha"), None, Some(b"a3-from-B")).unwrap();
    b.delete(&mkid("gamma")).unwrap();
    b.put(&mkid("epsilon"), None, Some(b"e1")).unwrap();
    flush_sub(&mut b);

    a.add_remote("b", &pull_url_sub(&b)).unwrap();
    b.add_remote("a", &pull_url_sub(&a)).unwrap();

    // A pulls B: alpha conflict, A picks Local.
    let status = a.pull("b").unwrap();
    let mut progress = match status {
        MergeStatus::Conflicted(p) => p,
        _ => panic!("expected conflict on alpha"),
    };
    let ids: Vec<EntryId> = progress.conflicts().iter().map(|c| c.id.clone()).collect();
    for id in ids {
        progress.pick(id, Side::Local).unwrap();
    }
    let resolution = progress.resolve().unwrap();
    a.merge(resolution).unwrap();

    // B pulls A.
    let status = b.pull("a").unwrap();
    if let MergeStatus::Conflicted(mut p) = status {
        let ids: Vec<EntryId> = p.conflicts().iter().map(|c| c.id.clone()).collect();
        for id in ids {
            p.pick(id, Side::Incoming).unwrap();
        }
        let resolution = p.resolve().unwrap();
        b.merge(resolution).unwrap();
    }

    // Convergence: every id should resolve to the same value on both sides.
    for id_str in ["alpha", "beta", "gamma", "delta", "epsilon"] {
        let id = mkid(id_str);
        let a_data = get_data(&a, id_str);
        let b_data = get_data(&b, id_str);
        assert_eq!(
            a_data, b_data,
            "diverged on {id}: A={a_data:?} B={b_data:?}"
        );
    }

    // Specific final values reflecting the operations:
    assert_eq!(get_data(&a, "alpha").as_deref(), Some(&b"a2-from-A"[..]));
    assert!(get_data(&a, "beta").is_none(), "A archived beta");
    assert!(get_data(&a, "gamma").is_none(), "B deleted gamma");
    assert_eq!(get_data(&a, "delta").as_deref(), Some(&b"d1"[..]));
    assert_eq!(get_data(&a, "epsilon").as_deref(), Some(&b"e1"[..]));
}
