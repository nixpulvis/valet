//! Layout-agnostic benches. Each bench body is defined once and
//! `generic_bench!` expands it into one criterion group that contains both
//! layouts, keyed as `<name>/SubmoduleLayout/<n>` and `<name>/SubdirLayout/<n>`
//! (each layout's [`stringify!`]'d ident). Using [`BenchmarkId::new(layout, n)`]
//! rather than a group per layout lets criterion render a two-column
//! comparison table across parameter `n`.

mod common;

use std::time::Duration;

use criterion::{Throughput, criterion_group, criterion_main};

use common::{CORPUS_SIZE, entry_id, new_id};
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::SubmoduleLayout;

/// Benchmark with all layouts: `SubmoduleLayout` and `SubdirLayout`.
/// The caller supplies the per-layout type-alias name via
/// `generic_bench!(name<L>, ...)`; see [`bench!`] for arm shapes.
macro_rules! generic_bench {
    ($name:ident<$lt:ident>, $($tt:tt)*) => {
        bench!($name, $($tt)* , layouts<$lt>: [SubmoduleLayout, SubdirLayout]);
    };
}

// Bulk-insert N entries into a fresh store.
generic_bench!(put_fresh<L>,
    setup: common::fresh(),
    throughput: |n| Throughput::Elements(n as u64),
    |store, n| {
        for i in 0..n {
            store
                .put(&entry_id(i), Some(b"label"), Some(b"payload"))
                .unwrap();
        }
    }
);

// Write N fresh entries onto a pre-populated corpus. Measures
// steady-state put cost once the store already holds CORPUS_SIZE
// entries.
generic_bench!(put_into_corpus<L>,
    setup: common::populated(CORPUS_SIZE),
    throughput: |n| Throughput::Elements(n as u64),
    |store, n| {
        for i in 0..n {
            store
                .put(&new_id(i), Some(b"label"), Some(b"payload"))
                .unwrap();
        }
    },
    measurement_time: Duration::from_secs(15),
);

// Read N entries from the pre-populated corpus. Cycles through ids
// modulo CORPUS_SIZE.
generic_bench!(get_from_corpus<L>,
    setup: common::populated(CORPUS_SIZE),
    throughput: |n| Throughput::Elements(n as u64),
    |store, n| {
        for i in 0..n {
            let _ = store
                .get(&entry_id(i % CORPUS_SIZE))
                .unwrap()
                .expect("live");
        }
    }
);

// Bundle the pre-populated corpus via `save`. `save` is non-destructive,
// so the handle is seeded once per n and reused across iterations.
generic_bench!(save_corpus<L>,
    seed: |n| common::populated::<L>(n),
    throughput: |n, _s| Throughput::Elements(n as u64),
    body: |h| h.store.save().unwrap(),
);

// Rehydrate a store from a `save()` tarball via `load` into a fresh
// scratch dir. Byte throughput is reported so submodule and subdir
// can be compared against the same pressure.
generic_bench!(load_corpus<L>,
    seed: |n| {
        let mut h = common::populated::<L>(n);
        h.store.save().expect("save")
    },
    throughput: |_n, blob| Throughput::Bytes(blob.len() as u64),
    setup: |blob, _n| {
        let scratch = tempfile::Builder::new()
            .prefix("storgit-bench-")
            .tempdir()
            .expect("tempdir");
        let path = scratch.path().join("repo");
        (scratch, path, blob.clone())
    },
    body: |(scratch, path, bytes)| {
        let store = storgit::Store::<L>::load(&bytes, path).expect("load");
        (store, scratch)
    },
    flat_threshold: 50,
);

// Open an already-persisted store. The on-disk repo is seeded once
// per n; each iteration just runs `Store::open` against that path.
generic_bench!(open_corpus<L>,
    seed: |n| {
        let h = common::populated::<L>(n);
        let scratch = h.scratch.expect("scratch");
        let path = scratch.path().join("repo");
        // Drop the seeding Store so any pending parent state is
        // flushed before we start opening against the path.
        drop(h.store);
        (scratch, path)
    },
    throughput: |n, _s| Throughput::Elements(n as u64),
    body: |(_scratch, path)| storgit::Store::<L>::open(path.clone()).expect("open"),
);

criterion_group!(
    benches,
    put_fresh,
    put_into_corpus,
    get_from_corpus,
    save_corpus,
    load_corpus,
    open_corpus,
);
criterion_main!(benches);
