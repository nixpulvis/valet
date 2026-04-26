//! Submodule-layout-specific benches: the [`Bundle`] envelope,
//! save/load bundling, and workloads that exercise the lazy-load
//! path via `load_module`.
//!
//! Layout-agnostic benches (put / get / n_put_fresh) live in
//! `benches/generic.rs` where they run against both layouts.
//!
//! ## Timing gotcha
//!
//! Routines here use `iter_batched` and always **return** the `Store` instead
//! of letting it drop at end-of-closure. Criterion collects the returned values
//! into a `Vec` and drops them after the timed section ends; if we instead
//! discarded the store inside the closure, [`tempfile::TempDir`]'s `Drop` would
//! run inside the timed window and charge the tear-down cost (walking and
//! unlinking a parent bare repo plus N submodule bare repos) to the routine.

mod common;

use std::collections::HashMap;
use std::time::Duration;

use criterion::{Throughput, criterion_group, criterion_main};
use storgit::{EntryId, Layout, Store, SubmoduleLayout, layout::submodule::Bundle};

use common::{Handle, entry_id, new_id};

/// In memory persistence layer: one byte blob per module plus the parent.
#[derive(Clone, Default)]
struct Storage {
    parent: Vec<u8>,
    modules: HashMap<EntryId, Vec<u8>>,
}

impl Storage {
    fn apply(&mut self, bundle: Bundle) {
        if !bundle.parent.is_empty() {
            self.parent = bundle.parent;
        }
        for (id, bytes) in bundle.modules {
            self.modules.insert(id, bytes);
        }
        for id in bundle.deleted {
            self.modules.remove(&id);
        }
    }

    fn metadata_only_bundle(&self) -> Bundle {
        Bundle {
            parent: self.parent.clone(),
            modules: HashMap::new(),
            deleted: Vec::new(),
        }
    }
}

/// Build a fresh store at a newly-allocated scratch path and apply
/// `bundle`. The `Handle` carries the scratch TempDir for lifetime
/// management.
fn new_with_bundle(bundle: Bundle, scratch: tempfile::TempDir) -> Handle<SubmoduleLayout> {
    let path = scratch.path().join("repo");
    let layout = SubmoduleLayout::new(path)
        .unwrap()
        .with_bundle(bundle)
        .unwrap();
    Handle {
        store: Store { layout },
        scratch: Some(scratch),
    }
}

fn new_with_bundle_tmp(bundle: Bundle) -> Handle<SubmoduleLayout> {
    new_with_bundle(bundle, tempdir())
}

fn load_bytes(bytes: &[u8]) -> Handle<SubmoduleLayout> {
    let scratch = tempdir();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::load(bytes, path).unwrap();
    Handle {
        store,
        scratch: Some(scratch),
    }
}

fn tempdir() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("storgit-bench-")
        .tempdir()
        .unwrap()
}

fn build_storage(n: usize) -> Storage {
    let mut h = common::fresh::<SubmoduleLayout>();
    for i in 0..n {
        h.store
            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
            .expect("populate put");
    }
    let mut storage = Storage::default();
    storage.apply(h.store.bundle().expect("bundle"));
    storage
}

fn build_blob(n: usize) -> Vec<u8> {
    let storage = build_storage(n);
    let bundle = Bundle {
        parent: storage.parent,
        modules: storage.modules,
        deleted: Vec::new(),
    };
    new_with_bundle_tmp(bundle).store.save().expect("save")
}

bench!(bench_new_with_bundle,
    seed: |n| build_storage(n),
    throughput: |n, _s| Throughput::Elements(n as u64),
    setup: |storage, _n| (storage.metadata_only_bundle(), tempdir()),
    body: |(bundle, path)| new_with_bundle(bundle, path),
    layouts<L>: [SubmoduleLayout],
);

bench!(bench_lazy_get,
    seed: |n| {
        let storage = build_storage(common::CORPUS_SIZE);
        let pairs: Vec<(EntryId, Vec<u8>)> = (0..n)
            .map(|i| {
                let id = entry_id(i % common::CORPUS_SIZE);
                let bytes = storage.modules.get(&id).cloned().expect("module row");
                (id, bytes)
            })
            .collect();
        (storage, pairs)
    },
    throughput: |n, _s| Throughput::Elements(n as u64),
    setup: |(storage, pairs), _n| {
        (new_with_bundle_tmp(storage.metadata_only_bundle()), pairs.clone())
    },
    body: |(h, pairs)| {
        for (id, bytes) in pairs {
            h.store.ensure_loaded(&id, Some(bytes)).expect("load");
            let _entry = h.store.get(&id).expect("get").expect("live");
        }
        h
    },
    layouts<L>: [SubmoduleLayout],
);

bench!(bench_bundle,
    seed: |_n| build_storage(common::CORPUS_SIZE),
    throughput: |n, _s| Throughput::Elements(n as u64),
    setup: |storage, n| {
        let mut h = new_with_bundle_tmp(storage.metadata_only_bundle());
        for i in 0..n {
            h.store
                .put(&new_id(i), Some(b"label"), Some(b"payload"))
                .unwrap();
        }
        h
    },
    body: |mut h| {
        let _ = h.store.bundle().unwrap();
        h
    },
    measurement_time: Duration::from_secs(12),
    layouts<L>: [SubmoduleLayout],
);

bench!(bench_n_put_1_bundle,
    setup: common::fresh(),
    throughput: |n| Throughput::Elements(n as u64),
    |h, n| {
        for i in 0..n {
            h.store
                .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                .unwrap();
        }
        let _ = h.store.bundle().unwrap();
    },
    layouts<L>: [SubmoduleLayout],
);

bench!(bench_n_put_n_bundle,
    setup: common::fresh(),
    throughput: |n| Throughput::Elements(n as u64),
    |h, n| {
        for i in 0..n {
            h.store
                .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                .unwrap();
            let _ = h.store.bundle().unwrap();
        }
    },
    measurement_time: Duration::from_secs(40),
    layouts<L>: [SubmoduleLayout],
);

bench!(bench_load,
    seed: |n| build_blob(n),
    throughput: |_n, blob| Throughput::Bytes(blob.len() as u64),
    setup: |blob, _n| blob.clone(),
    body: |bytes| load_bytes(&bytes),
    flat_threshold: 50,
    layouts<L>: [SubmoduleLayout],
);

bench!(bench_save,
    seed: |n| {
        let mut h = common::fresh::<SubmoduleLayout>();
        for i in 0..n {
            h.store
                .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                .unwrap();
        }
        h
    },
    throughput: |n, _s| Throughput::Elements(n as u64),
    body: |h| h.store.save().unwrap(),
    layouts<L>: [SubmoduleLayout],
);

criterion_group!(
    benches,
    bench_new_with_bundle,
    bench_lazy_get,
    bench_bundle,
    bench_n_put_1_bundle,
    bench_n_put_n_bundle,
    bench_load,
    bench_save,
);
criterion_main!(benches);
