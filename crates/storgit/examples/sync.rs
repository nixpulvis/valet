//! Walk through a two-store sync session: build a shared starting
//! state, fork into A and B, do divergent local edits on each, then
//! have A pull B and B pull A, resolving any conflicts.
//!
//!     cargo run -p storgit --example sync -- [--submodule|--subdir]
//!
//! Default layout is `--submodule`.

use storgit::layout::Layout;
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::SubmoduleLayout;
use storgit::merge::{MergeStatus, Side};
use storgit::{EntryId, Store};

enum LayoutChoice {
    Submodule,
    Subdir,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut layout = LayoutChoice::Submodule;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--submodule" => layout = LayoutChoice::Submodule,
            "--subdir" => layout = LayoutChoice::Subdir,
            other => return Err(format!("unknown flag: {other}").into()),
        }
    }

    let scratch = tempfile::Builder::new().prefix("storgit-sync-").tempdir()?;

    match layout {
        LayoutChoice::Submodule => run::<SubmoduleLayout>(scratch.path(), flush_submodule),
        LayoutChoice::Subdir => run::<SubdirLayout>(scratch.path(), flush_noop),
    }
}

fn run<L: Layout>(
    scratch: &std::path::Path,
    flush: fn(&mut Store<L>) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>>
where
    Store<L>: Pullable<L>,
{
    let alpha = EntryId::new("alpha")?;
    let beta = EntryId::new("beta")?;
    let gamma = EntryId::new("gamma")?;
    let delta = EntryId::new("delta")?;
    let epsilon = EntryId::new("epsilon")?;

    // ---- Build the seed and fork A / B from it via save / load.
    let seed_path = scratch.join("seed");
    let mut seed = Store::<L>::new(seed_path)?;
    seed.put(&alpha, None, Some(b"a1"))?;
    seed.put(&beta, None, Some(b"b1"))?;
    seed.put(&gamma, None, Some(b"g1"))?;
    flush(&mut seed)?;
    let blob = seed.save()?;
    println!("seed: alpha=a1 beta=b1 gamma=g1");
    println!();

    let a_path = scratch.join("a");
    let b_path = scratch.join("b");
    let mut a = Store::<L>::load(&blob, a_path)?;
    let mut b = Store::<L>::load(&blob, b_path)?;
    println!("cloned A and B from seed");
    println!();

    // ---- Diverge.
    a.put(&alpha, None, Some(b"a2-from-A"))?;
    a.archive(&beta)?;
    a.put(&delta, None, Some(b"d1"))?;
    flush(&mut a)?;
    println!("A: alpha:=a2-from-A, archive(beta), delta:=d1");
    print_state(&a, "A", &[&alpha, &beta, &gamma, &delta, &epsilon])?;

    b.put(&alpha, None, Some(b"a3-from-B"))?;
    b.delete(&gamma)?;
    b.put(&epsilon, None, Some(b"e1"))?;
    flush(&mut b)?;
    println!("B: alpha:=a3-from-B, delete(gamma), epsilon:=e1");
    print_state(&b, "B", &[&alpha, &beta, &gamma, &delta, &epsilon])?;

    // ---- Wire each as a remote of the other and pull both ways.
    a.add_remote("b", &file_url(&b))?;
    b.add_remote("a", &file_url(&a))?;

    println!("--- A pulls B ---");
    let status = a.pull_("b")?;
    handle(&mut a, status, "A", Side::Local)?;
    print_state(&a, "A", &[&alpha, &beta, &gamma, &delta, &epsilon])?;

    println!("--- B pulls A ---");
    let status = b.pull_("a")?;
    handle(&mut b, status, "B", Side::Local)?;
    print_state(&b, "B", &[&alpha, &beta, &gamma, &delta, &epsilon])?;

    Ok(())
}

/// Trait abstracting `pull` and `merge` across the two layouts so the
/// example can be written once. Both layouts have the same surface
/// today; they're just defined as inherent methods on each
/// `Store<L>`, so we wire them up here.
trait Pullable<L: Layout>: Sized {
    fn pull_(&mut self, remote: &str) -> Result<MergeStatus<L>, storgit::Error>;
    fn merge_(&mut self, resolution: storgit::MergeResolution<L>) -> Result<(), storgit::Error>;
    fn file_url(&self) -> String;
}

impl Pullable<SubmoduleLayout> for Store<SubmoduleLayout> {
    fn pull_(&mut self, remote: &str) -> Result<MergeStatus<SubmoduleLayout>, storgit::Error> {
        self.pull(remote)
    }
    fn merge_(
        &mut self,
        r: storgit::MergeResolution<SubmoduleLayout>,
    ) -> Result<(), storgit::Error> {
        self.merge(r).map(|_| ())
    }
    fn file_url(&self) -> String {
        format!("file://{}", self.git_dir().display())
    }
}

impl Pullable<SubdirLayout> for Store<SubdirLayout> {
    fn pull_(&mut self, remote: &str) -> Result<MergeStatus<SubdirLayout>, storgit::Error> {
        self.pull(remote)
    }
    fn merge_(&mut self, r: storgit::MergeResolution<SubdirLayout>) -> Result<(), storgit::Error> {
        self.merge(r).map(|_| ())
    }
    fn file_url(&self) -> String {
        format!("file://{}", self.git_dir().display())
    }
}

fn file_url<L: Layout>(store: &Store<L>) -> String
where
    Store<L>: Pullable<L>,
{
    Pullable::file_url(store)
}

/// Print the merge outcome and resolve any conflicts by picking
/// `our_pick` for every conflicting id.
fn handle<L: Layout>(
    store: &mut Store<L>,
    status: MergeStatus<L>,
    label: &str,
    our_pick: Side,
) -> Result<(), Box<dyn std::error::Error>>
where
    Store<L>: Pullable<L>,
{
    match status {
        MergeStatus::Clean(_) => {
            println!("  {label}: clean merge");
        }
        MergeStatus::Conflicted(mut progress) => {
            println!("  {label}: {} conflict(s):", progress.conflicts().len());
            for c in progress.conflicts() {
                println!(
                    "    {} (blob {:?}, local {} vs incoming {})",
                    c.id,
                    c.blob,
                    c.local.to_short_hex(),
                    c.incoming.to_short_hex(),
                );
            }
            let ids: Vec<EntryId> = progress.conflicts().iter().map(|c| c.id.clone()).collect();
            for id in ids {
                progress.pick(id, our_pick)?;
            }
            let resolution = progress.resolve()?;
            store.merge_(resolution)?;
            println!("  {label}: resolved by picking {our_pick:?} on every conflict");
        }
    }
    Ok(())
}

fn print_state<L: Layout>(
    store: &Store<L>,
    label: &str,
    ids: &[&EntryId],
) -> Result<(), Box<dyn std::error::Error>> {
    println!("  {label} state:");
    for id in ids {
        let entry = store.get(id)?;
        match entry {
            Some(e) => match &e.data {
                Some(bytes) => println!("    {id}: {}", show(bytes)),
                None => println!("    {id}: <empty>"),
            },
            None => println!("    {id}: <absent>"),
        }
    }
    println!();
    Ok(())
}

fn flush_submodule(store: &mut Store<SubmoduleLayout>) -> Result<(), Box<dyn std::error::Error>> {
    store.snapshot()?;
    Ok(())
}

fn flush_noop<L: Layout>(_: &mut Store<L>) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}

fn show(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => format!("<{} bytes>", bytes.len()),
    }
}
