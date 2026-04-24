//! Shared test helpers: id shorthand, put/get shortcuts, and the
//! per-layout factories invoked by `generic_test!` in `generic.rs`.
//!
//! Factories return a [`Handle`] that carries the [`Store`]
//! alongside any scratch [`tempfile::TempDir`] whose lifetime must
//! match it. `Handle<L>` derefs to `Store<L>` so callers treat it
//! like the underlying store.

#![allow(dead_code)]

use std::ops::{Deref, DerefMut};

use storgit::layout::Layout;
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::SubmoduleLayout;
use storgit::{Id, Store};

pub fn mkid(s: &str) -> Id {
    Id::new(s).unwrap()
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
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::new(path).unwrap();
    Handle {
        store,
        scratch: Some(scratch),
    }
}

pub fn make_subdir_store() -> Handle<SubdirLayout> {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubdirLayout>::new(path).unwrap();
    Handle {
        store,
        scratch: Some(scratch),
    }
}
