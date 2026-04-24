//! Submodule-layout-specific benches: persistence envelopes
//! ([`Parts`] / [`Snapshot`]), save/load bundling, and workloads
//! that exercise the lazy-load path via `load_module`.
//!
//! Layout-agnostic benches (put / get / n_put_fresh) live in
//! `benches/generic.rs` where they run against both layouts.
//!
//! ## Timing gotcha
//!
//! Routines here use `iter_batched` and always **return** the
//! `Store` instead of letting it drop at end-of-closure. Criterion
//! collects the returned values into a `Vec` and drops them after
//! the timed section ends; if we instead discarded the store inside
//! the closure, [`tempfile::TempDir`]'s `Drop` would run inside the
//! timed window and charge the tear-down cost (walking and
//! unlinking a parent bare repo plus N submodule bare repos) to the
//! routine.

mod common;

use std::collections::HashMap;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use storgit::layout::submodule::SubmoduleLayout;
use storgit::{Id, ModuleChange, Parts, Snapshot, Store};

use common::{Handle, MEASUREMENT_TIME, SCALING_NS, entry_id, new_id};

/// In memory persistence layer: one byte blob per module plus the parent.
#[derive(Clone, Default)]
struct Storage {
    parent: Vec<u8>,
    modules: HashMap<Id, Vec<u8>>,
}

impl Storage {
    fn apply(&mut self, snap: Snapshot) {
        if let Some(bytes) = snap.parent {
            self.parent = bytes;
        }
        for (id, change) in snap.modules {
            match change {
                ModuleChange::Changed(bytes) => {
                    self.modules.insert(id, bytes);
                }
                ModuleChange::Deleted => {
                    self.modules.remove(&id);
                }
            }
        }
    }

    fn metadata_only_parts(&self) -> Parts {
        Parts {
            parent: self.parent.clone(),
            modules: HashMap::new(),
            fetcher: None,
        }
    }
}

/// Build a fresh store at a newly-allocated scratch path and apply
/// `parts`. The `Handle` carries the scratch TempDir for lifetime
/// management.
fn open_with_parts(parts: Parts) -> Handle<SubmoduleLayout> {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-bench-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::new(path)
        .unwrap()
        .with_parts(parts)
        .unwrap();
    Handle {
        store,
        scratch: Some(scratch),
    }
}

fn load_bytes(bytes: &[u8]) -> Handle<SubmoduleLayout> {
    let scratch = tempfile::Builder::new()
        .prefix("storgit-bench-")
        .tempdir()
        .unwrap();
    let path = scratch.path().join("repo");
    let store = Store::<SubmoduleLayout>::load(bytes, path).unwrap();
    Handle {
        store,
        scratch: Some(scratch),
    }
}

fn build_storage_with(n: usize) -> Storage {
    let mut h = common::fresh::<SubmoduleLayout>();
    for i in 0..n {
        h.store
            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
            .expect("populate put");
    }
    let mut storage = Storage::default();
    storage.apply(h.store.snapshot().expect("snapshot"));
    storage
}

fn build_storage() -> Storage {
    build_storage_with(common::CORPUS_SIZE)
}

fn build_blob(n: usize) -> Vec<u8> {
    let mut h = common::fresh::<SubmoduleLayout>();
    for i in 0..n {
        h.store
            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
            .expect("populate put");
    }
    h.store.save().expect("save")
}

fn bench_open(c: &mut Criterion) {
    let mut group = c.benchmark_group("open");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        let storage = build_storage_with(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &storage, |b, storage| {
            b.iter_batched(
                || storage.metadata_only_parts(),
                open_with_parts,
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_lazy_get(c: &mut Criterion) {
    let mut group = c.benchmark_group("lazy_get");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    let storage = build_storage();
    for &n in SCALING_NS {
        let pairs: Vec<(Id, Vec<u8>)> = (0..n)
            .map(|i| {
                let id = entry_id(i % common::CORPUS_SIZE);
                let bytes = storage.modules.get(&id).cloned().expect("module row");
                (id, bytes)
            })
            .collect();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter_batched(
                || (open_with_parts(storage.metadata_only_parts()), pairs.clone()),
                |(mut h, pairs)| {
                    for (id, bytes) in pairs {
                        h.store.load_module(id.clone(), bytes);
                        let _entry = h.store.get(&id).expect("get").expect("live");
                    }
                    h
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    let storage = build_storage();
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut h = open_with_parts(storage.metadata_only_parts());
                    for i in 0..n {
                        h.store
                            .put(&new_id(i), Some(b"label"), Some(b"payload"))
                            .unwrap();
                    }
                    h
                },
                |mut h| {
                    let _ = h.store.snapshot().unwrap();
                    h
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_n_put_1_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("n_put_1_snapshot");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                common::fresh::<SubmoduleLayout>,
                |mut h| {
                    for i in 0..n {
                        h.store
                            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                            .unwrap();
                    }
                    let _ = h.store.snapshot().unwrap();
                    h
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_n_put_n_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("n_put_n_snapshot");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                common::fresh::<SubmoduleLayout>,
                |mut h| {
                    for i in 0..n {
                        h.store
                            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                            .unwrap();
                        let _ = h.store.snapshot().unwrap();
                    }
                    h
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_load(c: &mut Criterion) {
    let mut group = c.benchmark_group("load");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        let blob = build_blob(n);
        group.throughput(Throughput::Bytes(blob.len() as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &blob, |b, blob| {
            b.iter_batched(
                || blob.clone(),
                |bytes| load_bytes(&bytes),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

fn bench_save(c: &mut Criterion) {
    let mut group = c.benchmark_group("save");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut h = common::fresh::<SubmoduleLayout>();
            for i in 0..n {
                h.store
                    .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                    .unwrap();
            }
            b.iter(|| h.store.save().unwrap());
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_open,
    bench_lazy_get,
    bench_snapshot,
    bench_n_put_1_snapshot,
    bench_n_put_n_snapshot,
    bench_load,
    bench_save,
);
criterion_main!(benches);
