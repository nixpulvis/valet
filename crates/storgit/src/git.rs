use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::{Commit, Tree, tree::Entry as TreeEntry, tree::EntryKind};

use crate::error::Error;

/// Branch that both parent and submodules commit to.
pub(crate) const BRANCH: &str = "refs/heads/main";
/// Filename in an entry's tree carrying the payload bytes. Shared
/// across layouts: subdir uses `records/<id>/data`; submodule uses
/// `data` at the root of each per-id repo.
pub(crate) const DATA_FILE: &str = "data";
/// Filename in an entry's tree carrying the searchable label bytes.
/// Empty or absent means no label. Like [`DATA_FILE`], shared across
/// layouts.
pub(crate) const LABEL_FILE: &str = "label";
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

/// Finalise a merge: write `editor`'s tree, write a merge commit
/// over `parents`, and update `ref_path` to point at it. Returns the
/// new commit id. Caller is responsible for `MERGE_HEAD` lifecycle
/// and any post-write rebuild.
pub(crate) fn write_merge_commit(
    repo: &gix::Repository,
    mut editor: gix::object::tree::Editor<'_>,
    parents: Vec<gix::ObjectId>,
    ref_path: &Path,
) -> Result<gix::ObjectId, Error> {
    let tree = editor
        .write()
        .map_err(|e| Error::Git(Box::new(e)))?
        .detach();
    let commit = write_commit(repo, tree, parents, "merge")?;
    write_ref_file(ref_path, commit)?;
    Ok(commit)
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

/// Path-scoped handle on a bare git repo. Groups the small helpers
/// that operate on a repo's on-disk files: its canonical branch ref
/// ([`refs/heads/main`][BRANCH]), its `MERGE_HEAD` marker, and its
/// `HEAD` symref. Nothing here opens `gix::Repository` — these are the
/// cheap, filesystem-level ops that shouldn't pay to construct one.
#[derive(Clone, Copy)]
pub(crate) struct BareRepo<'a> {
    path: &'a Path,
}

impl<'a> BareRepo<'a> {
    pub(crate) fn new(path: &'a Path) -> Self {
        Self { path }
    }

    /// Path to the `refs/heads/main` file inside the repo.
    pub(crate) fn head_ref(&self) -> PathBuf {
        self.path.join("refs").join("heads").join("main")
    }

    /// Read the canonical branch HEAD. `None` when no commit has
    /// landed on the branch yet.
    pub(crate) fn read_head(&self) -> Result<Option<gix::ObjectId>, Error> {
        read_ref_file(&self.head_ref())
    }

    /// Overwrite the canonical branch HEAD to point at `oid`.
    pub(crate) fn write_head(&self, oid: gix::ObjectId) -> Result<(), Error> {
        write_ref_file(&self.head_ref(), oid)
    }

    /// Path to the `MERGE_HEAD` marker file.
    pub(crate) fn merge_head_path(&self) -> PathBuf {
        self.path.join("MERGE_HEAD")
    }

    /// Read `MERGE_HEAD` if present.
    pub(crate) fn read_merge_head(&self) -> Result<Option<gix::ObjectId>, Error> {
        read_ref_file(&self.merge_head_path())
    }

    /// Write `MERGE_HEAD` containing `oid`. Standard git convention:
    /// presence of this file signals "merge in progress."
    pub(crate) fn write_merge_head(&self, oid: gix::ObjectId) -> Result<(), Error> {
        std::fs::write(self.merge_head_path(), format!("{oid}\n"))?;
        Ok(())
    }

    /// Read `MERGE_HEAD`, erroring if absent. `context` is included in
    /// the error message to disambiguate which merge state was missing.
    pub(crate) fn require_merge_head(&self, context: &str) -> Result<gix::ObjectId, Error> {
        self.read_merge_head()?
            .ok_or_else(|| Error::Other(format!("merge: {context}")))
    }

    /// Delete `MERGE_HEAD` if present; not-found is a no-op.
    pub(crate) fn clear_merge_head(&self) -> Result<(), Error> {
        match std::fs::remove_file(self.merge_head_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// True when a merge is in progress (`MERGE_HEAD` present).
    pub(crate) fn merge_in_progress(&self) -> bool {
        self.read_merge_head().map(|o| o.is_some()).unwrap_or(false)
    }

    /// Verify that the repo has its `HEAD` pointing at storgit's
    /// canonical [`BRANCH`]. `context` is prefixed to error messages
    /// so callers can identify which layout rejected the repo.
    pub(crate) fn validate_head_branch(&self, context: &str) -> Result<(), Error> {
        let head_raw = std::fs::read_to_string(self.path.join("HEAD"))
            .map_err(|e| Error::Other(format!("{context}: cannot read HEAD: {e}")))?;
        let head_trimmed = head_raw.trim();
        let expected = format!("ref: {BRANCH}");
        if head_trimmed != expected {
            return Err(Error::Other(format!(
                "{context}: HEAD must be {expected:?}; got {head_trimmed:?}"
            )));
        }
        Ok(())
    }
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

/// Build a per-entry tree's blob list for the canonical
/// `(data, label)` slot shape. Each slot's oid comes from
/// `Some(bytes)` (written as a new blob) or falls back to the blob at
/// that filename in `prior_entries`; when neither produces an oid the
/// slot is omitted.
///
/// Returned entries are in `DATA_FILE` then `LABEL_FILE` order, which
/// already satisfies git's strict filename sort, so callers can feed
/// the list straight into a [`Tree`] without resorting.
pub(crate) fn build_slot_entries(
    repo: &gix::Repository,
    prior_entries: Option<&[TreeEntry]>,
    label: Option<&[u8]>,
    data: Option<&[u8]>,
) -> Result<Vec<TreeEntry>, Error> {
    let prior_blob = |filename: &str| -> Option<gix::ObjectId> {
        prior_entries.and_then(|entries| {
            entries
                .iter()
                .find(|e| e.filename.as_bstr() == BStr::new(filename))
                .map(|e| e.oid)
        })
    };
    let mut entries: Vec<TreeEntry> = Vec::with_capacity(2);
    let data_oid = match data {
        Some(bytes) => Some(repo.write_blob(bytes)?.detach()),
        None => prior_blob(DATA_FILE),
    };
    if let Some(oid) = data_oid {
        entries.push(TreeEntry {
            mode: EntryKind::Blob.into(),
            filename: BString::from(DATA_FILE),
            oid,
        });
    }
    let label_oid = match label {
        Some(bytes) => Some(repo.write_blob(bytes)?.detach()),
        None => prior_blob(LABEL_FILE),
    };
    if let Some(oid) = label_oid {
        entries.push(TreeEntry {
            mode: EntryKind::Blob.into(),
            filename: BString::from(LABEL_FILE),
            oid,
        });
    }
    Ok(entries)
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
