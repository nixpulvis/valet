//! Shared bench helpers: sweep sizes, id generators, layout-generic
//! [`Handle`] factories. Mirrors `tests/common/mod.rs` -- `Handle<L>`
//! carries a [`Store`] plus any scratch [`tempfile::TempDir`] whose
//! lifetime must match it, and derefs to the store.

#![allow(dead_code)]

use std::ops::{Deref, DerefMut};
use std::time::Duration;

use storgit::Id;
use storgit::Store;
use storgit::layout::Layout;

/// Spacing from tiny lot (10 entries) to medium lot (250).
pub const SCALING_NS: &[usize] = &[10, 25, 50, 100, 250];

/// Per-group measurement budget. The larger N points need more than
/// criterion's default 5s to collect `sample_size(10)` cleanly.
pub const MEASUREMENT_TIME: Duration = Duration::from_secs(30);

/// Fixed corpus size for benches whose swept parameter is operation
/// count rather than corpus size.
pub const CORPUS_SIZE: usize = 100;

pub fn entry_id(i: usize) -> Id {
    Id::new(format!("entry-{i:06}")).expect("id")
}

/// Id namespace distinct from [`entry_id`] so benches can put fresh
/// entries onto a pre-populated corpus without collisions.
pub fn new_id(i: usize) -> Id {
    Id::new(format!("new-{i:06}")).expect("id")
}

pub struct Handle<L: Layout> {
    pub store: Store<L>,
    pub scratch: Option<tempfile::TempDir>,
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

/// Fresh, empty store of layout `L` under a new scratch dir.
pub fn fresh<L: Layout>() -> Handle<L> {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-bench-")
        .tempdir()
        .expect("tempdir");
    let path = scratch.path().join("repo");
    let store = Store::<L>::new(path).expect("open");
    Handle {
        store,
        scratch: Some(scratch),
    }
}

/// Fresh store pre-loaded with `n` entries.
pub fn populated<L: Layout>(n: usize) -> Handle<L> {
    let mut h = fresh::<L>();
    for i in 0..n {
        h.put(&entry_id(i), Some(b"label"), Some(b"payload"))
            .expect("populate put");
    }
    h
}
