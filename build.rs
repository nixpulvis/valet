fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_FFI");
    if std::env::var("CARGO_FEATURE_FFI").is_ok() {
        valet_build::generate_header("valet");
    }
}
