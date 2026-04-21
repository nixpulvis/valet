//! Socket path resolution for the daemon and its clients.

use std::path::PathBuf;

/// The socket path the daemon binds to and clients connect to. Honors
/// `$VALET_SOCKET` if set; otherwise falls back to [`default_path`].
/// Use this in preference to `default_path` so the env override is
/// applied uniformly across the daemon, shim, and FFI clients.
pub fn path() -> PathBuf {
    std::env::var_os("VALET_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(default_path)
}

/// Default socket path: `$XDG_RUNTIME_DIR/valet/valet.sock`, falling back to
/// `$TMPDIR/valet/valet.sock` (and `/tmp/valet/valet.sock` if `TMPDIR` is
/// unset). Returning an absolute path; the parent directory is not created
/// here.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("TMPDIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("valet").join("valet.sock")
}
