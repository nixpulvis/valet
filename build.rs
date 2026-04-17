fn main() {
    if std::env::var("CARGO_FEATURE_FFI").is_ok() {
        valet_build::generate_header("valet");
    }
}
