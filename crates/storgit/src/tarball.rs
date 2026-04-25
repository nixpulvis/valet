use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use crate::error::Error;

/// Extract tarball bytes into `dest`.
pub(crate) fn untar_into(bytes: &[u8], dest: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dest)?;
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    archive.unpack(dest)?;
    Ok(())
}

/// Tar the contents of `dir` into a deterministic uncompressed archive.
pub(crate) fn tar_dir(dir: &Path) -> Result<Vec<u8>, Error> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        builder.mode(tar::HeaderMode::Deterministic);
        builder.follow_symlinks(false);
        builder.append_dir_all(".", dir)?;
        builder.finish()?;
    }
    Ok(buf)
}

/// Copy every `objects/**` entry from a bare-repo `tarball` into
/// `dest_git_dir/objects/`, creating directories as needed. Returns
/// the oid parsed from the tarball's `refs/heads/main`, or `None` if
/// the tarball has no such ref. Everything outside `objects/` and
/// `refs/heads/main` (config, HEAD, packed-refs, .gitmodules, ...) is
/// ignored; those describe the source repo, not the caller's.
///
/// Git objects are content-addressed, so an oid that already exists
/// locally is a byte-identical no-op. Used by the submodule layout's
/// `apply` path to fold an incoming `Parts` bundle into the local
/// object DB without standing up a scratch dir and a throwaway
/// remote to `git fetch` from.
pub(crate) fn import_tarball_objects(
    tarball: &[u8],
    dest_git_dir: &Path,
) -> Result<Option<gix::ObjectId>, Error> {
    let objects_root = dest_git_dir.join("objects");
    std::fs::create_dir_all(&objects_root)?;
    let mut head: Option<gix::ObjectId> = None;
    let mut archive = tar::Archive::new(Cursor::new(tarball));
    for entry in archive.entries()? {
        let mut entry = entry?;
        let rel: PathBuf = entry.path()?.to_path_buf();
        let components: Vec<String> = rel
            .components()
            .filter_map(|c| match c {
                std::path::Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();

        if components.first().map(String::as_str) == Some("objects") {
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let dst = dest_git_dir.join(&rel);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if dst.exists() {
                // Git objects are content-addressed; identical oid
                // means identical bytes. Skip to avoid reopening.
                continue;
            }
            entry.unpack(&dst)?;
            continue;
        }

        if components.as_slice() == ["refs", "heads", "main"] {
            let mut buf = String::new();
            entry.read_to_string(&mut buf)?;
            let trimmed = buf.trim();
            if !trimmed.is_empty() {
                head = Some(gix::ObjectId::from_hex(trimmed.as_bytes()).map_err(|e| {
                    Error::Other(format!("import: invalid refs/heads/main {trimmed:?}: {e}"))
                })?);
            }
            continue;
        }

        // All other entries (HEAD, config, packed-refs, .gitmodules,
        // hooks, info, description) are the source repo's metadata
        // and must not overwrite the destination's.
    }
    Ok(head)
}
