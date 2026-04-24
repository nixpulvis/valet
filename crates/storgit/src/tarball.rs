use std::io::Cursor;
use std::path::Path;

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
