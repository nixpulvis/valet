//! `valetd` - the Valet daemon binary.
//!
//! Placeholder. The real server will own the vault, accept connections on a
//! Unix socket, and serve the `valetd` wire protocol. Until then, clients
//! (including the macOS AutoFill extension) use the in-process stub exposed
//! by this crate's `stub` feature; see `valetd::stub`.

fn main() {
    unimplemented!("valetd daemon not implemented yet; use the stub feature for development")
}
