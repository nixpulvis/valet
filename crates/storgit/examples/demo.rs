//! Walk through a basic storgit session: put a couple of entries, update
//! one in place so it picks up a second commit, archive an entry, and
//! print the history of each. Also demonstrates the split-snapshot
//! persistence model: each `snapshot()` reports only the parts that
//! changed since the previous snapshot. Run with:
//!
//!     cargo run -p storgit --example demo

use storgit::layout::submodule::ModuleChange;
use storgit::{Id, Store};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scratch = tempfile::Builder::new().prefix("storgit-demo-").tempdir()?;
    let mut store = Store::<storgit::layout::submodule::SubmoduleLayout>::new(
        scratch.path().join("repo"),
    )?;
    println!("opened empty store");

    let github = Id::new("0194a3c1-1111-7000-8000-000000000001")?;
    let email = Id::new("0194a3c1-2222-7000-8000-000000000002")?;
    let temp = Id::new("0194a3c1-3333-7000-8000-000000000003")?;

    store.put(&github, None, Some(b"hunter2"))?;
    println!("put {github}    = hunter2");
    store.put(&email, None, Some(b"correct horse"))?;
    println!("put {email}    = correct horse");
    store.put(&temp, None, Some(b"random"))?;
    println!("put {temp}    = random");
    report_snapshot("puts", &mut store)?;

    store.put(&github, None, Some(b"hunter3"))?;
    println!("put {github}    = hunter3  (update)");
    report_snapshot("update", &mut store)?;

    store.archive(&email)?;
    println!("archive {email}");
    report_snapshot("archive", &mut store)?;

    store.delete(&temp)?;
    println!("delete {temp}");
    report_snapshot("delete", &mut store)?;

    println!("live entries: {:?}", store.list()?);
    println!();

    report_store(&store, &[&github, &email])?;

    let blob = store.save()?;
    println!("saved store");
    let load_scratch = tempfile::Builder::new()
        .prefix("storgit-demo-load-")
        .tempdir()?;
    let loaded = Store::<storgit::layout::submodule::SubmoduleLayout>::load(
        &blob,
        load_scratch.path().join("repo"),
    )?;
    println!("loaded saved store");
    report_store(&loaded, &[&github, &email])?;

    Ok(())
}

/// Take a snapshot and print which parts a persistence layer would
/// rewrite. Demonstrates that untouched modules never appear.
fn report_snapshot(label: &str, store: &mut Store) -> Result<(), Box<dyn std::error::Error>> {
    let snap = store.snapshot()?;
    print!("  snapshot [{label}]:");
    if snap.parent.is_some() {
        print!(" parent");
    }
    for (name, change) in &snap.modules {
        match change {
            ModuleChange::Changed(_) => print!(" {name}"),
            ModuleChange::Deleted => print!(" -{name}"),
        }
    }
    println!();
    Ok(())
}

fn report_store(store: &Store, ids: &[&Id]) -> Result<(), Box<dyn std::error::Error>> {
    for id in ids {
        println!("history of {id}:");
        for entry in store.history(id)? {
            match &entry.data {
                Some(bytes) => println!("  {} {}", short(&entry.commit.0), show(bytes)),
                None => println!("  {} <archived>", short(&entry.commit.0)),
            }
        }
        println!();
    }

    Ok(())
}

fn short(bytes: &[u8]) -> String {
    bytes.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn show(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => format!("<{} bytes>", bytes.len()),
    }
}
