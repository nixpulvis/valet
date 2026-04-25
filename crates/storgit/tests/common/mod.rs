//! Shared test helpers: id shorthand, put/get shortcuts, and the
//! per-layout factories invoked by `generic_test!` in `generic.rs`.
//!
//! Factories return a [`Handle`] that carries the [`Store`]
//! alongside any scratch [`tempfile::TempDir`] whose lifetime must
//! match it. `Handle<L>` derefs to `Store<L>` so callers treat it
//! like the underlying store.

#![allow(dead_code)]

use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};

use storgit::layout::Layout;
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::{Parts, SubmoduleLayout};
use storgit::{EntryId, Store};
use tempfile::TempDir;

pub fn mkid(s: &str) -> EntryId {
    EntryId::new(s).unwrap()
}

pub fn put_data<L: Layout>(store: &mut Store<L>, id_str: &str, data: &[u8]) {
    store.put(&mkid(id_str), None, Some(data)).unwrap();
}

pub fn get_data<L: Layout>(store: &Store<L>, id_str: &str) -> Option<Vec<u8>> {
    store.get(&mkid(id_str)).unwrap().and_then(|e| e.data)
}

/// Owns a [`Store`] and any scratch directory whose lifetime should
/// end with the test. Drops the scratch dir (and its contents) when
/// the handle drops.
pub struct Handle<L: Layout> {
    store: Store<L>,
    scratch: Option<tempfile::TempDir>,
    path: PathBuf,
}

impl<L: Layout> Handle<L> {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Drop the store but keep the scratch dir alive, and return the
    /// path the store was rooted at. Useful for testing `Store::open`
    /// on an already-populated path.
    pub fn into_path(self) -> (PathBuf, Option<tempfile::TempDir>) {
        let Handle { path, scratch, .. } = self;
        (path, scratch)
    }
}

impl<L: Layout> Deref for Handle<L> {
    type Target = Store<L>;
    fn deref(&self) -> &Store<L> {
        &self.store
    }
}

impl<L: Layout> DerefMut for Handle<L> {
    fn deref_mut(&mut self) -> &mut Store<L> {
        &mut self.store
    }
}

pub fn make_submodule_store() -> Handle<SubmoduleLayout> {
    make_store::<SubmoduleLayout>()
}

pub fn make_subdir_store() -> Handle<SubdirLayout> {
    make_store::<SubdirLayout>()
}

pub fn make_store<L: Layout>() -> Handle<L> {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<L>::new(path.clone()).unwrap();
    Handle {
        store,
        scratch: Some(scratch),
        path,
    }
}

/// Create a fresh store of the same layout as `other`. The `_other`
/// argument is only for type inference at the call site.
pub fn make_store_like<L: Layout>(_other: &Handle<L>) -> Handle<L> {
    make_store::<L>()
}

/// Ensure every pending write is flushed to refs on disk. No-op for
/// layouts that commit on every `put`; snapshots the parent for
/// submodule.
impl Handle<SubdirLayout> {
    pub fn flush_for_test(&mut self) {}
}

impl Handle<SubmoduleLayout> {
    pub fn flush_for_test(&mut self) {
        self.store.snapshot().unwrap();
    }
}

/// Fresh, empty submodule-layout store under a newly-allocated
/// scratch dir. The TempDir is returned so the caller keeps it alive
/// for the test's scope.
pub fn fresh_submodule() -> (TempDir, Store<SubmoduleLayout>) {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::new(path).unwrap();
    (scratch, store)
}

/// Fresh submodule store under a new scratch dir with `parts` applied
/// via the builder.
pub fn open_with_parts(parts: Parts) -> (TempDir, Store<SubmoduleLayout>) {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::new(path)
        .unwrap()
        .with_parts(parts)
        .unwrap();
    (scratch, store)
}

/// Rehydrate a submodule store from a `save()` tarball under a fresh
/// scratch dir.
pub fn load_submodule_bytes(bytes: &[u8]) -> (TempDir, Store<SubmoduleLayout>) {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::load(bytes, path).unwrap();
    (scratch, store)
}

/// Recursive byte-size of a directory tree.
pub fn dir_size(path: &Path) -> std::io::Result<u64> {
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

/// Count the loose objects under a `objects/` directory.
pub fn count_loose_objects(objects_root: &Path) -> usize {
    let mut n = 0;
    let Ok(dir) = std::fs::read_dir(objects_root) else {
        return 0;
    };
    for entry in dir.flatten() {
        let fname = entry.file_name();
        let s = fname.to_string_lossy();
        if s.len() == 2
            && s.chars().all(|c| c.is_ascii_hexdigit())
            && let Ok(sub) = std::fs::read_dir(entry.path())
        {
            n += sub.flatten().count();
        }
    }
    n
}

/// Decompress and untar a snap-framed tarball into a fresh tempdir.
pub fn extract_to_tmp(bytes: &[u8]) -> TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let mut tarball = Vec::new();
    std::io::copy(&mut snap::read::FrameDecoder::new(bytes), &mut tarball).unwrap();
    tar::Archive::new(std::io::Cursor::new(tarball))
        .unpack(tmp.path())
        .unwrap();
    tmp
}
