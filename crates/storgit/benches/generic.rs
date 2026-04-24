//! Layout-agnostic benches. Each bench body is defined once and
//! `generic_bench!` expands it into one criterion group per layout.
//! Output groups are `<name>/submodule` and `<name>/subdir` so
//! criterion reports the two layouts side by side.

mod common;

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};

use common::{CORPUS_SIZE, MEASUREMENT_TIME, SCALING_NS, entry_id, new_id};
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::SubmoduleLayout;

macro_rules! generic_bench {
    ($name:ident, $setup:expr, |$store:ident, $n:ident| $body:block) => {
        fn $name(c: &mut Criterion) {
            fn run<L: storgit::layout::Layout>(c: &mut Criterion, layout_name: &str) {
                let name = format!("{}/{}", stringify!($name), layout_name);
                let mut group = c.benchmark_group(&name);
                group.sample_size(10);
                group.measurement_time(MEASUREMENT_TIME);
                for &$n in SCALING_NS {
                    group.throughput(Throughput::Elements($n as u64));
                    group.bench_with_input(BenchmarkId::from_parameter($n), &$n, |bench, &$n| {
                        #[allow(unused_mut)]
                        bench.iter_batched(
                            || $setup,
                            |mut $store: common::Handle<L>| {
                                $body;
                                $store
                            },
                            BatchSize::LargeInput,
                        );
                    });
                }
                group.finish();
            }
            run::<SubmoduleLayout>(c, "submodule");
            run::<SubdirLayout>(c, "subdir");
        }
    };
}

// Bulk-insert N entries into a fresh store.
generic_bench!(put_fresh, common::fresh(), |store, n| {
    for i in 0..n {
        store
            .put(&entry_id(i), Some(b"label"), Some(b"payload"))
            .unwrap();
    }
});

// Write N fresh entries onto a pre-populated corpus. Measures
// steady-state put cost once the store already holds CORPUS_SIZE
// entries.
generic_bench!(put_into_corpus, common::populated(CORPUS_SIZE), |store, n| {
    for i in 0..n {
        store
            .put(&new_id(i), Some(b"label"), Some(b"payload"))
            .unwrap();
    }
});

// Read N entries from the pre-populated corpus. Cycles through ids
// modulo CORPUS_SIZE.
generic_bench!(get_from_corpus, common::populated(CORPUS_SIZE), |store, n| {
    for i in 0..n {
        let _ = store
            .get(&entry_id(i % CORPUS_SIZE))
            .unwrap()
            .expect("live");
    }
});

criterion_group!(benches, put_fresh, put_into_corpus, get_from_corpus);
criterion_main!(benches);
