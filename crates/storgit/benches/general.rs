//! Caller-visible storgit workloads, sized for the realistic range.
//!
//! Eight groups, one per primary use case. The first six mirror
//! valet's intended usage of the library — `Store::open` from
//! [`Parts`] (with `modules` empty so only the parent untarss),
//! lazy [`Store::load_module`] when an entry is touched,
//! [`Store::snapshot`] for incremental persistence:
//!
//! 1. `open`: cold open of an existing N-entry store. Just the
//!    parent is untarred; the label index is ready immediately.
//! 2. `get`: N `(load_module, get)` pairs against an opened lazy
//!    store backed by the static-size corpus.
//! 3. `put`: write N fresh entries onto the static-size corpus. No
//!    snapshot; that's its own group.
//! 4. `snapshot`: take one snapshot on a store that has N pending
//!    puts on top of the static-size corpus. The cost of flushing
//!    N dirty modules to persistence in one shot.
//! 5. `n_put_1_snapshot`: bulk-insert N entries into a fresh store
//!    and snapshot once at the end. Amortised import path.
//! 6. `n_put_n_snapshot`: build N entries one `put + snapshot` at a
//!    time on a fresh store. Worst case for storage churn — every
//!    entry triggers a parent + module re-tar.
//!
//! The remaining two cover the all-in-one save/load path used by
//! callers that want a single self-contained tarball:
//!
//! 7. `load`: rehydrate a store from a saved tarball.
//! 8. `save`: serialise a store back to a single tarball.
//!
//! For `get`, `put`, and `snapshot` the open cost lives in the
//! (untimed) setup closure, so the routine measures only the
//! on-demand work.
//!
//! Each group sweeps [`SCALING_NS`].
//!
//! Filter with `cargo bench -p storgit --bench general -- <group>`.
//!
//! ## Timing gotcha
//!
//! Routines here use `iter_batched` and always **return** the
//! `Store` instead of letting it drop at end-of-closure. Criterion
//! collects the returned values into a `Vec` and drops them after
//! the timed section ends; if we instead discarded the store inside
//! the closure, [`tempfile::TempDir`]'s `Drop` would run inside the
//! timed window and charge the tear-down cost (walking and
//! unlinking a parent bare repo plus N submodule bare repos, each
//! with ~14 files of `init_bare` scaffolding) to the routine. That
//! once made `marginal_put`-style benches look O(N) when the work
//! is actually flat.

use std::collections::HashMap;
use std::time::Duration;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use storgit::{Id, ModuleChange, Parts, Snapshot, Store};

/// Spacing from tiny lot (10 entries) to medium lot (250).
const SCALING_NS: &[usize] = &[10, 25, 50, 100, 250];

/// Per-group measurement budget. The larger N points (100, 250) need
/// more than criterion's default 5s to collect `sample_size(10)` cleanly.
const MEASUREMENT_TIME: Duration = Duration::from_secs(30);

fn entry_id(i: usize) -> Id {
    Id::new(format!("entry-{i:06}")).expect("id")
}

/// Id namespace distinct from [`entry_id`] so benches can put fresh
/// entries onto a corpus built from `entry_id` without collisions.
fn new_id(i: usize) -> Id {
    Id::new(format!("new-{i:06}")).expect("id")
}

/// Stand-in for valet's persistence layer: one byte blob per module
/// plus the parent. Mirrors what the library will write to SQLite (one
/// row per module, one row for the parent bundle).
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

    /// Parts with just the parent populated — `modules` intentionally
    /// empty so `Store::open` doesn't untar anything it doesn't need.
    fn metadata_only_parts(&self) -> Parts {
        Parts {
            parent: self.parent.clone(),
            modules: HashMap::new(),
            fetcher: None,
        }
    }
}

/// Fixed corpus size for benches that read from a pre-populated
/// `Storage`. Held constant so each bench's swept parameter `n`
/// measures operation count, not corpus size.
const CORPUS_SIZE: usize = 100;

/// Build a `Storage` populated with [`CORPUS_SIZE`] entries, each
/// with a small label and payload. The fixed corpus is what most
/// benches read from when the swept parameter is operation count
/// rather than corpus size.
fn build_storage() -> Storage {
    build_storage_with(CORPUS_SIZE)
}

/// Same as [`build_storage`] but with a caller-chosen corpus size.
/// Used by benches whose parameter actually is the corpus size
/// (e.g. `open`).
fn build_storage_with(n: usize) -> Storage {
    let mut store = Store::new().expect("store");
    for i in 0..n {
        store
            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
            .expect("populate put");
    }
    let mut storage = Storage::default();
    storage.apply(store.snapshot().expect("snapshot"));
    storage
}

/// Build a populated store and return the bytes of a full `save()`.
fn build_blob(n: usize) -> Vec<u8> {
    let mut store = Store::new().expect("store");
    for i in 0..n {
        store
            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
            .expect("populate put");
    }
    store.save().expect("save")
}

/// Use case 1: cold open of an existing lot with N entries already
/// persisted, carrying just enough state for a working label index.
/// Sweeps N to show how open scales with corpus size; the parent
/// untar is the only work that grows.
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
                |parts| Store::open(parts).expect("open"),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Use case 2: do N `(load_module, get)` pairs against an opened
/// lazy store backed by the static-size corpus. When N exceeds the
/// corpus size we cycle through ids modulo the corpus (still real
/// untar work for each iteration's first N corpus accesses, then
/// no-ops for repeats — gives a sense of how lookup cost amortises).
fn bench_get_from_100(c: &mut Criterion) {
    let mut group = c.benchmark_group("get");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    let storage = build_storage();
    for &n in SCALING_NS {
        let pairs: Vec<(Id, Vec<u8>)> = (0..n)
            .map(|i| {
                let id = entry_id(i % CORPUS_SIZE);
                let bytes = storage.modules.get(&id).cloned().expect("module row");
                (id, bytes)
            })
            .collect();
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter_batched(
                || {
                    let store = Store::open(storage.metadata_only_parts()).expect("open");
                    (store, pairs.clone())
                },
                |(mut store, pairs)| {
                    for (id, bytes) in pairs {
                        store.load_module(id.clone(), bytes);
                        let _entry = store.get(&id).expect("get").expect("live");
                    }
                    store
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Use case 3: write N fresh entries onto the static-size corpus.
/// The open lives in the setup closure (not timed); the routine
/// measures N puts only, no snapshot.
fn bench_put_from_100(c: &mut Criterion) {
    let mut group = c.benchmark_group("put");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    let storage = build_storage();
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || Store::open(storage.metadata_only_parts()).expect("open"),
                |mut store| {
                    for i in 0..n {
                        store
                            .put(&new_id(i), Some(b"label"), Some(b"payload"))
                            .unwrap();
                    }
                    store
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Use case 4: snapshot a store that has N pending puts on top of
/// the static-size corpus. The open and the puts live in the setup
/// closure (not timed); the routine measures `snapshot` only —
/// flushing the parent tree and re-tarring the parent plus the N
/// touched modules.
fn bench_snapshot_from_100(c: &mut Criterion) {
    let mut group = c.benchmark_group("snapshot");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    let storage = build_storage();
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || {
                    let mut store = Store::open(storage.metadata_only_parts()).expect("open");
                    for i in 0..n {
                        store
                            .put(&new_id(i), Some(b"label"), Some(b"payload"))
                            .unwrap();
                    }
                    store
                },
                |mut store| {
                    let _ = store.snapshot().unwrap();
                    store
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Use case 5: bulk-insert N entries into a fresh store and take one snapshot
/// at the end. Amortised cost when the caller can defer persistence to a single
/// flush. This mirrors an import process.
fn bench_n_put_1_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("n_put_1_snapshot");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || Store::new().expect("store"),
                |mut store| {
                    for i in 0..n {
                        store
                            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                            .unwrap();
                    }
                    let _ = store.snapshot().unwrap();
                    store
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Use case 6: build N entries one `put + snapshot` at a time on a
/// fresh store. Worst case for storage churn — every entry triggers
/// a parent + module re-tar.
fn bench_n_put_n_snapshot(c: &mut Criterion) {
    let mut group = c.benchmark_group("n_put_n_snapshot");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            b.iter_batched(
                || Store::new().expect("store"),
                |mut store| {
                    for i in 0..n {
                        store
                            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                            .unwrap();
                        let _ = store.snapshot().unwrap();
                    }
                    store
                },
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Use case 5: rehydrate a store from a self-contained `save()`
/// tarball. Untars every module up front, so cost grows with N.
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
                |bytes| Store::load(&bytes).expect("load"),
                BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// Use case 6: serialise a populated store to a single tarball.
/// Force-loads every module before tarring, so cost grows with N.
fn bench_save(c: &mut Criterion) {
    let mut group = c.benchmark_group("save");
    group.sample_size(10);
    group.measurement_time(MEASUREMENT_TIME);
    for &n in SCALING_NS {
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, &n| {
            let mut store = Store::new().expect("store");
            for i in 0..n {
                store
                    .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                    .unwrap();
            }
            b.iter(|| store.save().unwrap());
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_open,
    bench_get_from_100,
    bench_put_from_100,
    bench_snapshot_from_100,
    bench_n_put_1_snapshot,
    bench_n_put_n_snapshot,
    bench_load,
    bench_save,
);
criterion_main!(benches);
