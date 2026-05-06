//! Walk through a basic storgit session: put a couple of entries, update
//! one in place so it picks up a second commit, archive an entry, and
//! print the history of each. For [`SubmoduleLayout`] also demonstrates
//! the split-bundle persistence model: each `bundle()` reports only
//! the parts that changed since the previous bundle. Run with:
//!
//!     cargo run -p storgit --example demo -- [--submodule|--subdir]
//!
//! Default layout is `--submodule`.

use std::path::PathBuf;

use storgit::{
    EntryId, Store,
    layout::{Layout, subdir::SubdirLayout, submodule::SubmoduleLayout},
};

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

    let scratch = tempfile::Builder::new().prefix("storgit-demo-").tempdir()?;
    let load_scratch = tempfile::Builder::new()
        .prefix("storgit-demo-load-")
        .tempdir()?;

    match layout {
        LayoutChoice::Submodule => run::<SubmoduleLayout>(
            scratch.path().join("repo"),
            load_scratch.path().join("repo"),
            report_bundle_submodule,
        ),
        LayoutChoice::Subdir => run::<SubdirLayout>(
            scratch.path().join("repo"),
            load_scratch.path().join("repo"),
            |_| Ok(()),
        ),
    }
}

fn run<L: Layout>(
    path: PathBuf,
    load_path: PathBuf,
    mut report: impl FnMut(&mut Store<L>) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut store = Store::<L>::new(path)?;
    println!("opened empty store");

    let github = EntryId::new("github")?;
    let email = EntryId::new("email")?;
    let temp = EntryId::new("temp")?;

    store.put(&github, None, Some(b"hunter2"))?;
    println!("put {github} = hunter2");
    store.put(&email, None, Some(b"correct horse"))?;
    println!("put {email}  = correct horse");
    store.put(&temp, None, Some(b"random"))?;
    println!("put {temp}   = random");
    report(&mut store)?;

    store.put(&github, None, Some(b"hunter3"))?;
    println!("put {github} = hunter3  (update)");
    report(&mut store)?;

    store.archive(&email)?;
    println!("archive {email}");
    report(&mut store)?;

    store.delete(&temp)?;
    println!("delete {temp}");
    report(&mut store)?;

    println!("live entries: {:?}", store.list()?);
    println!();

    report_store(&store, &[&github, &email])?;

    let blob = store.save()?;
    println!("saved store ({:.1} KB)", blob.len() as f64 / 1024.0);
    let loaded = Store::<L>::load(&blob, load_path)?;
    println!("loaded saved store");
    report_store(&loaded, &[&github, &email])?;

    Ok(())
}

/// Take a bundle and print which parts a persistence layer would
/// rewrite. Demonstrates that untouched modules never appear.
fn report_bundle_submodule(
    store: &mut Store<SubmoduleLayout>,
) -> Result<(), Box<dyn std::error::Error>> {
    let bundle = store.bundle()?;
    print!("  bundle:");
    if !bundle.parent.is_empty() {
        print!(" parent");
    }
    for name in bundle.modules.keys() {
        print!(" {name}");
    }
    for name in &bundle.deleted {
        print!(" -{name}");
    }
    println!();
    Ok(())
}

fn report_store<L: Layout>(
    store: &Store<L>,
    ids: &[&EntryId],
) -> Result<(), Box<dyn std::error::Error>> {
    for id in ids {
        println!("history of {id}:");
        for entry in store.history(id)? {
            match &entry.data {
                Some(bytes) => println!("  {} {}", entry.commit.to_short_hex(), show(bytes)),
                None => println!("  {} <archived>", entry.commit.to_short_hex()),
            }
        }
        println!();
    }

    Ok(())
}

fn show(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => format!("<{} bytes>", bytes.len()),
    }
}
