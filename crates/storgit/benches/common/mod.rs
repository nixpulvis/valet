//! Shared bench helpers: sweep sizes, id generators, layout-generic [`Handle`]
//! factories. Mirrors `tests/common/mod.rs` -- `Handle<L>` carries a [`Store`]
//! plus any scratch [`tempfile::TempDir`] whose lifetime must match it, and
//! derefs to the store.

#![allow(dead_code)]

use std::ops::{Deref, DerefMut};
use std::time::Duration;

use criterion::{BenchmarkGroup, SamplingMode, measurement::WallTime};
use storgit::EntryId;
use storgit::Store;
use storgit::layout::Layout;

/// Layout-parameterized bench definition. Each arm expands into one
/// `fn $name(c: &mut ::criterion::Criterion)` whose `benchmark_group` sweeps
/// `SCALING_NS` across every layout in the `layouts<...>:` clause,
/// keying each benchmark as `<name>/<layout_label>/<n>`. See the
/// per-arm comments below for the three supported shapes.
///
/// Optional `flat_threshold: <expr>` overrides the default
/// [`FLAT_SAMPLING_THRESHOLD`] for benches whose per-iteration cost
/// grows faster than the default assumes.
///
/// Optional `measurement_time: <expr>` (a [`Duration`]) overrides the
/// default [`MEASUREMENT_TIME`] for benches whose per-iteration cost
/// doesn't fit in the default budget even under flat sampling.
///
/// `layouts<L>: [<LayoutIdent>, ...]` lists the layouts to bench
/// against; `L` (or any ident the caller picks) is introduced as a
/// type alias for the current layout inside every per-layout block,
/// so seed/setup/body expressions can name the layout type directly
/// (e.g. `common::populated::<L>(n)`). Each layout's
/// [`stringify!`]'d ident becomes its label in the criterion output.
#[macro_export]
macro_rules! bench {
    // Fresh-per-iter: `setup` runs every iteration and produces a
    // fresh `common::Handle<L>`; the body operates on it. The handle
    // is returned from the routine so its `TempDir` drop lands
    // outside the timed section.
    ($name:ident,
     setup: $setup:expr,
     throughput: |$tn:ident| $thru:expr,
     |$store:ident, $n:ident| $body:block
     $(, flat_threshold: $ft:expr)?
     $(, measurement_time: $mt:expr)?
     $(,)?
     , layouts<$lt:ident>: [$($lty:ident),+ $(,)?]
     $(,)?
    ) => {
        fn $name(c: &mut ::criterion::Criterion) {
            let mut group = c.benchmark_group(stringify!($name));
            group.sample_size(10);
            group.measurement_time($crate::meas!($($mt)?));
            let threshold = $crate::thresh!($($ft)?);
            $({
                #[allow(dead_code)]
                type $lt = $lty;
                for &$n in $crate::common::SCALING_NS {
                    $crate::common::set_sampling_mode(&mut group, $n, threshold);
                    {
                        let $tn = $n;
                        group.throughput($thru);
                    }
                    group.bench_with_input(
                        ::criterion::BenchmarkId::new(stringify!($lty), $n), &$n,
                        |bench, &$n| {
                            #[allow(unused_mut)]
                            bench.iter_batched(
                                || $setup,
                                |mut $store: $crate::common::Handle<$lt>| {
                                    $body;
                                    $store
                                },
                                ::criterion::BatchSize::LargeInput,
                            );
                        });
                }
            })+
            group.finish();
        }
    };

    // Per-n seed, no per-iter setup: `seed` builds a per-layout,
    // per-n value once; `body` is called each iteration with `&mut
    // seed`. Implemented via `bench.iter` so the `&mut seed` reborrow
    // doesn't escape a setup closure.
    ($name:ident,
     seed: |$sn:ident| $seed:expr,
     throughput: |$tn:ident, $ts:pat_param| $thru:expr,
     body: |$bi:pat_param| $body:expr
     $(, flat_threshold: $ft:expr)?
     $(, measurement_time: $mt:expr)?
     $(,)?
     , layouts<$lt:ident>: [$($lty:ident),+ $(,)?]
     $(,)?
    ) => {
        fn $name(c: &mut ::criterion::Criterion) {
            let mut group = c.benchmark_group(stringify!($name));
            group.sample_size(10);
            group.measurement_time($crate::meas!($($mt)?));
            let threshold = $crate::thresh!($($ft)?);
            $({
                #[allow(dead_code)]
                type $lt = $lty;
                for &$sn in $crate::common::SCALING_NS {
                    #[allow(unused_mut)]
                    let mut seed_val = { $seed };
                    $crate::common::set_sampling_mode(&mut group, $sn, threshold);
                    {
                        let $tn = $sn;
                        let $ts = &seed_val;
                        group.throughput($thru);
                    }
                    group.bench_function(
                        ::criterion::BenchmarkId::new(stringify!($lty), $sn),
                        |bench| {
                            bench.iter(|| {
                                let $bi = &mut seed_val;
                                $body
                            });
                        });
                }
            })+
            group.finish();
        }
    };

    // Per-n seed, per-iter setup: adds a `setup:` clause whose
    // expression runs every iteration with access to `&mut seed` and
    // `n`, producing the batched value passed to `body`. Use when an
    // iteration needs fresh state derived from the seed (e.g. a
    // scratch dir for `load`).
    ($name:ident,
     seed: |$sn:ident| $seed:expr,
     throughput: |$tn:ident, $ts:pat_param| $thru:expr,
     setup: |$up_s:pat_param, $up_n:ident| $setup:expr,
     body: |$bi:pat_param| $body:expr
     $(, flat_threshold: $ft:expr)?
     $(, measurement_time: $mt:expr)?
     $(,)?
     , layouts<$lt:ident>: [$($lty:ident),+ $(,)?]
     $(,)?
    ) => {
        fn $name(c: &mut ::criterion::Criterion) {
            let mut group = c.benchmark_group(stringify!($name));
            group.sample_size(10);
            group.measurement_time($crate::meas!($($mt)?));
            let threshold = $crate::thresh!($($ft)?);
            $({
                #[allow(dead_code)]
                type $lt = $lty;
                for &$sn in $crate::common::SCALING_NS {
                    #[allow(unused_mut)]
                    let mut seed_val = { $seed };
                    $crate::common::set_sampling_mode(&mut group, $sn, threshold);
                    {
                        let $tn = $sn;
                        let $ts = &seed_val;
                        group.throughput($thru);
                    }
                    group.bench_function(
                        ::criterion::BenchmarkId::new(stringify!($lty), $sn),
                        |bench| {
                            bench.iter_batched(
                                || {
                                    let $up_s = &mut seed_val;
                                    let $up_n = $sn;
                                    $setup
                                },
                                |$bi| $body,
                                ::criterion::BatchSize::LargeInput,
                            );
                        });
                }
            })+
            group.finish();
        }
    };
}

/// Spacing from tiny lot (10 entries) to medium lot (250).
pub const SCALING_NS: &[usize] = &[10, 25, 50, 100, 250];

/// N above this threshold switches the enclosing criterion group to
/// [`SamplingMode::Flat`]. Above ~50 entries per-iteration cost
/// climbs past what criterion's default linear sampling can fit into
/// its time budget, so we pick flat sampling automatically via
/// [`set_sampling_mode`].
pub const FLAT_SAMPLING_THRESHOLD: usize = 50;

/// Default criterion measurement window. Bumped above criterion's 5s
/// default so the slower SubmoduleLayout benches finish their 10
/// samples without warning; benches with even heavier per-iteration
/// cost override this via the `measurement_time:` macro clause.
pub const MEASUREMENT_TIME: Duration = Duration::from_secs(10);

/// Fixed corpus size for benches whose swept parameter is operation
/// count rather than corpus size.
pub const CORPUS_SIZE: usize = 100;

/// Pick a sampling mode for the given `n` and apply it to `group`.
/// Call this before each `bench_*` invocation inside a per-`n` loop.
/// `threshold` is the `n` at and above which the group switches to
/// [`SamplingMode::Flat`]; pass [`FLAT_SAMPLING_THRESHOLD`] for the
/// default, or a smaller value for benches whose per-iteration cost
/// grows faster than the default assumes (e.g. `load`, where even
/// small `n` overruns criterion's linear-sampling time budget).
pub fn set_sampling_mode(group: &mut BenchmarkGroup<'_, WallTime>, n: usize, threshold: usize) {
    let mode = if n >= threshold {
        SamplingMode::Flat
    } else {
        SamplingMode::Auto
    };
    group.sampling_mode(mode);
}

pub fn entry_id(i: usize) -> EntryId {
    EntryId::new(format!("entry-{i:06}")).expect("id")
}

/// EntryId namespace distinct from [`entry_id`] so benches can put fresh
/// entries onto a pre-populated corpus without collisions.
pub fn new_id(i: usize) -> EntryId {
    EntryId::new(format!("new-{i:06}")).expect("id")
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

/// Resolve the flat-sampling threshold for a [`bench!`] arm,
/// defaulting to [`FLAT_SAMPLING_THRESHOLD`] when the caller omits
/// the `flat_threshold:` clause.
#[macro_export]
macro_rules! thresh {
    () => {
        $crate::common::FLAT_SAMPLING_THRESHOLD
    };
    ($ft:expr) => {
        $ft
    };
}

/// Resolve the measurement-time override for a [`bench!`] arm,
/// defaulting to [`MEASUREMENT_TIME`] when the caller omits the
/// `measurement_time:` clause.
#[macro_export]
macro_rules! meas {
    () => {
        $crate::common::MEASUREMENT_TIME
    };
    ($mt:expr) => {
        $mt
    };
}
