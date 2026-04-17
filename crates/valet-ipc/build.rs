use std::{env, path::PathBuf};

fn main() {
    let crate_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let crate_dir = PathBuf::from(crate_dir);

    let out = crate_dir.join("include").join("valet_ipc.h");

    let config = cbindgen::Config::from_file(crate_dir.join("cbindgen.toml"))
        .expect("failed to read cbindgen.toml");

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&out);
        }
        Err(err) => {
            // Don't fail the build if cbindgen can't run (e.g. on docs.rs). The
            // header is checked in so downstream consumers don't depend on it.
            println!("cargo:warning=cbindgen failed: {err}");
        }
    }

    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=cbindgen.toml");
    println!("cargo:rerun-if-changed=build.rs");
}
