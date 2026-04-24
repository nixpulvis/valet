use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gix::bstr::BString;
use gix::objs::{Commit, Tree, tree::Entry as TreeEntry};

use crate::error::Error;

/// Branch that both parent and submodules commit to.
pub(crate) const BRANCH: &str = "refs/heads/main";
/// Identity used for every commit storgit writes. Storgit is a single-
/// writer library; the author is an implementation detail, not metadata
/// the caller can control.
pub(crate) const AUTHOR_NAME: &str = "storgit";
pub(crate) const AUTHOR_EMAIL: &str = "storgit@localhost";

/// Initialise a full bare repo with its own object database. Used
/// for the parent and for every submodule.
///
/// `gix::init_bare` respects git's `init.defaultBranch` config, so we
/// pin HEAD to [`BRANCH`] afterwards. Also prunes the template junk
/// (`hooks/*.sample`, `info/exclude`, `description`) that would
/// otherwise bloat the tarball.
pub(crate) fn init_bare_on_branch(path: &Path) -> Result<(), Error> {
    gix::init_bare(path)?;
    std::fs::write(path.join("HEAD"), format!("ref: {BRANCH}\n"))?;
    for junk in ["hooks", "info", "description"] {
        let p = path.join(junk);
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else if p.is_file() {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// Build and write a commit with storgit's canonical author/committer
/// and the current wall-clock time.
pub(crate) fn write_commit(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    parents: Vec<gix::ObjectId>,
    message: &str,
) -> Result<gix::ObjectId, Error> {
    let sig = current_signature();
    let commit = Commit {
        tree,
        parents: parents.into(),
        // TODO: support user info here.
        author: sig.clone(),
        committer: sig,
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    Ok(repo.write_object(&commit)?.detach())
}

/// Storgit's canonical signature, timestamped to now.
pub(crate) fn current_signature() -> gix::actor::Signature {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    gix::actor::Signature {
        name: AUTHOR_NAME.into(),
        email: AUTHOR_EMAIL.into(),
        time: gix::date::Time { seconds, offset: 0 },
    }
}

/// Path to the `refs/heads/main` file inside a bare repo.
pub(crate) fn module_ref_path(repo_path: &Path) -> PathBuf {
    repo_path.join("refs").join("heads").join("main")
}

/// Read a loose ref file and parse it as an object id. Returns `None`
/// if the file doesn't exist, which means "no commit on this branch yet".
pub(crate) fn read_ref_file(path: &Path) -> Result<Option<gix::ObjectId>, Error> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let trimmed = s.trim();
            let oid = gix::ObjectId::from_hex(trimmed.as_bytes())
                .map_err(|e| Error::Other(format!("invalid ref {path:?}: {e}")))?;
            Ok(Some(oid))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write a loose ref file for `oid`, creating parent directories if
/// needed. Mirrors git's own on-disk format for a branch head: the
/// 40-char hex SHA followed by a newline.
pub(crate) fn write_ref_file(path: &Path, oid: gix::ObjectId) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{oid}\n"))?;
    Ok(())
}

/// Read and decode a tree object by id into an owned [`Tree`].
// TODO: Do we really need to eager load all of this?
pub(crate) fn decode_tree(repo: &gix::Repository, id: gix::ObjectId) -> Result<Tree, Error> {
    let object = repo.find_object(id)?;
    let tree = object
        .try_into_tree()
        .map_err(|e| Error::Other(format!("expected tree: {e:?}")))?;
    // `Tree` borrows from `object`'s buffer, but we want an owned copy so
    // we can mutate it. `decode()` returns the borrowed view; we rebuild
    // the owned form directly from that.
    let decoded = tree.decode()?;
    Ok(Tree {
        entries: decoded
            .entries
            .iter()
            .map(|e| TreeEntry {
                mode: e.mode,
                filename: BString::from(e.filename.to_vec()),
                oid: e.oid.into(),
            })
            .collect(),
    })
}

/// Relative path within an `objects.git/` directory where git stores
/// a loose object: the first two hex characters of the SHA form a
/// shard directory, the remaining 38 are the filename. `abc1234...`
/// lives at `objects/ab/c1234...`.
pub(crate) fn loose_object_path(objects_dir: &Path, oid: gix::ObjectId) -> PathBuf {
    let hex = oid.to_string();
    let (shard, rest) = hex.split_at(2);
    objects_dir.join("objects").join(shard).join(rest)
}

/// Delete a loose object file if present; not-found is a no-op.
pub(crate) fn drop_loose_object(objects_dir: &Path, oid: gix::ObjectId) -> Result<(), Error> {
    let path = loose_object_path(objects_dir, oid);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}
