//! Shared build-script helpers.
//!
//! [`generate_header`] drives cbindgen for any workspace crate that exposes a
//! C-ABI surface. All settings common to every FFI header live in
//! `cbindgen.toml` alongside this crate; crate-specific bits (include guard,
//! preprocessor defines derived from Cargo features) are applied below.
//!
//! The header lands in two places:
//!
//! * `target/<triple>/<profile>/include/<name>.h` - profile-specific; what a
//!   dependent Rust crate would see via the emitted `cargo:include=...`.
//! * `target/<triple>/include/<name>.h` - profile-agnostic; what the macOS
//!   extension's Swift modulemap points at so one fixed path works for both
//!   debug and release builds.

use std::{env, fs, path::PathBuf};

pub fn generate_header(name: &str) {
    let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));

    // OUT_DIR = target/<triple>/<profile>/build/<pkg>-<hash>/out
    let profile_dir = out_dir
        .ancestors()
        .nth(3)
        .expect("OUT_DIR has unexpected shape")
        .to_path_buf();
    let triple_dir = profile_dir
        .parent()
        .expect("profile dir has parent")
        .to_path_buf();

    let include_dir = profile_dir.join("include");
    let shared_include_dir = triple_dir.join("include");
    fs::create_dir_all(&include_dir).expect("create include dir");
    fs::create_dir_all(&shared_include_dir).expect("create shared include dir");

    let header_name = format!("{name}.h");
    let header = include_dir.join(&header_name);
    let shared_header = shared_include_dir.join(&header_name);

    let shared_config = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("cbindgen.toml");
    let mut config = cbindgen::Config::from_file(&shared_config)
        .expect("failed to read valet-build cbindgen.toml");
    config.include_guard = Some(format!("{}_H", name.to_uppercase()));

    // Map each crate's `stub` Cargo feature to a matching header macro so the
    // Swift side can `#if <NAME>_FFI_STUB` without a second configuration
    // channel.
    let stub_macro = format!("{}_FFI_STUB", name.to_uppercase());
    config
        .defines
        .insert("feature = stub".to_string(), stub_macro);

    match cbindgen::Builder::new()
        .with_crate(&crate_dir)
        .with_config(config)
        .generate()
    {
        Ok(bindings) => {
            bindings.write_to_file(&header);
            fs::copy(&header, &shared_header).expect("copy header to shared include dir");
        }
        Err(err) => {
            println!("cargo:warning=cbindgen failed for {name}: {err}");
        }
    }

    println!("cargo:include={}", include_dir.display());
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed={}", shared_config.display());
    println!("cargo:rerun-if-changed=build.rs");
}
