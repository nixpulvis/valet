//! Create a small storgit store, save it, and extract it to a real
//! directory on disk so you can poke at it with the `git` CLI.
//!
//!     cargo run -p storgit --release --example explore [path]
//!
//! Default path is `./storgit-explore`. The directory is wiped and
//! recreated each run. After it prints the path, try:
//!
//!     cd <path>/parent.git && git log --oneline
//!     cd <path>/parent.git && git ls-tree HEAD
//!     cd <path>/modules/<id>.git && git log --oneline
//!     cd <path>/modules/<id>.git && git show HEAD:data
//!
//! where `<id>` is one of the UUIDs printed under "Live entries".
//!
//! Each repo (parent and every module) is a full self-contained bare
//! repo with its own object database, so `git` commands work inside
//! any of them independently.

use std::io::Cursor;
use std::path::PathBuf;

use storgit::{Id, Store};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target: PathBuf = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "./storgit-explore".into())
        .into();

    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }
    std::fs::create_dir_all(&target)?;

    let mut store = Store::new()?;

    // A handful of entries covering the interesting states: new,
    // updated (history), archived (tombstone), deleted.
    let github = Id::new("0194a3c1-1111-7000-8000-000000000001")?;
    let email = Id::new("0194a3c1-2222-7000-8000-000000000002")?;
    let scratch = Id::new("0194a3c1-3333-7000-8000-000000000003")?;
    let old_key = Id::new("0194a3c1-4444-7000-8000-000000000004")?;

    store.put(&github, Some(b"github"), Some(b"hunter2"))?;
    store.put(&github, Some(b"github"), Some(b"hunter3"))?; // updates github -> two commits
    store.put(&github, Some(b"github.com"), None)?; // label-only update, data blob reused

    store.put(
        &email,
        Some(b"nix@example.com"),
        Some(b"correct horse battery staple"),
    )?;

    store.put(&scratch, Some(b"scratch"), Some(b"throwaway"))?;
    store.archive(&scratch)?; // tombstone commit on scratch, dropped from list

    store.put(&old_key, Some(b"old-key"), Some(b"oldvalue"))?;
    store.delete(&old_key)?; // hard delete; submodule dir goes away

    let bytes = store.save()?;
    tar::Archive::new(Cursor::new(bytes)).unpack(&target)?;

    let abs = target.canonicalize()?;
    println!("storgit store extracted at:\n  {}", abs.display());
    println!();
    println!("Try:");
    println!("  cd {}/parent.git && git log --oneline", abs.display());
    println!("  cd {}/parent.git && git ls-tree HEAD", abs.display());
    println!(
        "  cd {}/modules/{github}.git && git log --oneline",
        abs.display()
    );
    println!(
        "  cd {}/modules/{github}.git && git show HEAD:data",
        abs.display()
    );
    println!("  cd {}/parent.git && git count-objects -v", abs.display());
    println!();
    println!("Live entries via storgit::list():");
    for name in store.list()? {
        println!("  {name}");
    }

    Ok(())
}
