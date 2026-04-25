use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gix::bstr::{BStr, ByteSlice};

use crate::error::Error;
use crate::git::{DATA_FILE, LABEL_FILE, decode_tree};
use crate::id::CommitId;

/// A single historical version of an entry.
///
/// Every live commit carries a `(label, data)` pair inside the module's tree; a
/// tombstone commit written by [`archive`] has an empty tree and surfaces as
/// `label = None, data = None`.
///
/// [`archive`]: crate::Store::archive
#[derive(Debug, Clone)]
pub struct Entry {
    /// Commit that produced this version of the entry.
    pub commit: CommitId,
    /// Committer timestamp of the commit.
    pub time: SystemTime,
    /// Caller-defined metadata the caller wants to scan cheaply
    /// without reading the full record. `None` on a tombstone or
    /// when no label has ever been written for this entry.
    pub label: Option<Vec<u8>>,
    /// The entry's payload bytes. `None` on a tombstone or when only
    /// a label has been written for this entry.
    pub data: Option<Vec<u8>>,
}

impl Entry {
    /// Read `label` and `data` blobs from `tree_id`. Either slot is
    /// `Some` when the corresponding filename exists in the tree,
    /// `None` otherwise. Used by both the per-id tree (submodule) and
    /// a `records/<id>/` subtree (subdir).
    pub(crate) fn read_slots(
        repo: &gix::Repository,
        tree_id: gix::ObjectId,
    ) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), Error> {
        let tree = decode_tree(repo, tree_id)?;
        let mut label = None;
        let mut data = None;
        for entry in tree.entries {
            if entry.filename.as_bstr() == BStr::new(DATA_FILE) {
                data = Some(repo.find_object(entry.oid)?.data.clone());
            } else if entry.filename.as_bstr() == BStr::new(LABEL_FILE) {
                label = Some(repo.find_object(entry.oid)?.data.clone());
            }
        }
        Ok((label, data))
    }

    /// Build an [`Entry`] for the commit at `commit_id`, reading its
    /// slots from `tree_id`. The tree may be the commit's own root
    /// tree (submodule) or a `records/<id>/` subtree (subdir); a
    /// `None` tree (an archived id at this commit) surfaces as both
    /// slots `None`.
    pub(crate) fn at_commit_tree(
        repo: &gix::Repository,
        commit_id: gix::ObjectId,
        tree_id: Option<gix::ObjectId>,
    ) -> Result<Self, Error> {
        let commit = repo.find_object(commit_id)?.into_commit();
        let sig = commit.committer()?;
        let seconds = sig.seconds().max(0) as u64;
        let time = UNIX_EPOCH + Duration::from_secs(seconds);
        let (label, data) = match tree_id {
            Some(t) => Self::read_slots(repo, t)?,
            None => (None, None),
        };
        Ok(Entry {
            commit: commit_id.into(),
            time,
            label,
            data,
        })
    }
}
