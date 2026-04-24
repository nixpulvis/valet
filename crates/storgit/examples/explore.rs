//! Create a small storgit store directly on disk so you can poke at
//! it with the `git` CLI.
//!
//!     cargo run -p storgit --release --example explore -- [--submodule|--subdir] [path]
//!
//! Default layout is `--submodule`; default path is
//! `./storgit-explore`. The target path must not already exist.
//!
//! For `--submodule` the on-disk shape is a parent bare repo plus
//! one bare submodule per entry; for `--subdir` it's one bare repo
//! whose tree carries every entry under `records/<id>/`. Each repo
//! is a full self-contained git repo, so `git` commands work inside
//! any of them independently.

use std::path::PathBuf;

use storgit::layout::Layout;
use storgit::layout::subdir::SubdirLayout;
use storgit::layout::submodule::SubmoduleLayout;
use storgit::{EntryId, Store};

enum LayoutChoice {
    Submodule,
    Subdir,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut layout = LayoutChoice::Submodule;
    let mut target: Option<PathBuf> = None;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--submodule" => layout = LayoutChoice::Submodule,
            "--subdir" => layout = LayoutChoice::Subdir,
            other if other.starts_with("--") => {
                return Err(format!("unknown flag: {other}").into());
            }
            other => target = Some(other.into()),
        }
    }
    let target = target.unwrap_or_else(|| "./storgit-explore".into());

    match layout {
        LayoutChoice::Submodule => {
            let mut store = Store::<SubmoduleLayout>::new(target.clone())?;
            run(&mut store)?;
        }
        LayoutChoice::Subdir => {
            let mut store = Store::<SubdirLayout>::new(target.clone())?;
            run(&mut store)?;
        }
    }

    Ok(())
}

fn run<L: Layout>(store: &mut Store<L>) -> Result<(), Box<dyn std::error::Error>> {
    populate(store)?;
    print_live_entries(&*store)?;
    Ok(())
}

/// Put a handful of entries covering the interesting states: new,
/// updated (history), archived (tombstone), deleted.
fn populate<L: Layout>(store: &mut Store<L>) -> Result<(), Box<dyn std::error::Error>> {
    let github = EntryId::new("0194a3c1-1111-7000-8000-000000000001")?;
    let email = EntryId::new("0194a3c1-2222-7000-8000-000000000002")?;
    let scratch = EntryId::new("0194a3c1-3333-7000-8000-000000000003")?;
    let old_key = EntryId::new("0194a3c1-4444-7000-8000-000000000004")?;

    store.put(&github, Some(b"github"), Some(b"hunter2"))?;
    store.put(&github, Some(b"github"), Some(b"hunter3"))?; // two commits now
    store.put(&github, Some(b"github.com"), None)?; // label-only, data blob reused

    store.put(
        &email,
        Some(b"nix@example.com"),
        Some(b"correct horse battery staple"),
    )?;

    store.put(&scratch, Some(b"scratch"), Some(b"throwaway"))?;
    store.archive(&scratch)?; // tombstone commit, dropped from list

    store.put(&old_key, Some(b"old-key"), Some(b"oldvalue"))?;
    store.delete(&old_key)?; // hard delete (submodule) / archive-like (subdir)

    Ok(())
}

fn print_live_entries<L: Layout>(store: &Store<L>) -> Result<(), Box<dyn std::error::Error>> {
    println!("Live entries via storgit::list():");
    for name in store.list()? {
        println!("  {name}");
    }
    Ok(())
}
