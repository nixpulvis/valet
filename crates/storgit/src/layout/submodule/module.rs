use gix::bstr::{BStr, ByteSlice};
use gix::objs::Tree;

use crate::entry::Entry;
use crate::error::Error;
use crate::git::{BareRepo, LABEL_FILE, build_slot_entries, decode_tree, write_commit};

/// Handle on an opened submodule bare repo. Groups the per-module
/// commit and tree-reading helpers so callers pass the repo in once
/// rather than threading `&gix::Repository` (or, worse, `&Path`)
/// through every call.
pub(crate) struct ModuleRepo<'r> {
    repo: &'r gix::Repository,
}

impl<'r> ModuleRepo<'r> {
    pub(crate) fn new(repo: &'r gix::Repository) -> Self {
        Self { repo }
    }

    fn bare(&self) -> BareRepo<'_> {
        BareRepo::new(self.repo.path())
    }

    /// Write a tombstone commit (empty tree), chaining it onto the
    /// module's current HEAD. New objects land in the module's own
    /// object DB; the module's `refs/heads/main` is updated.
    pub(crate) fn write_tombstone(&self) -> Result<gix::ObjectId, Error> {
        let br = self.bare();
        let tree = Tree {
            entries: Vec::new(),
        };
        let tree_id = self.repo.write_object(&tree)?.detach();
        let prior_commit = br.read_head()?;
        let commit_id = write_commit(
            self.repo,
            tree_id,
            prior_commit.into_iter().collect(),
            "archive",
        )?;
        br.write_head(commit_id)?;
        Ok(commit_id)
    }

    /// Write a commit whose tree contains up to one blob per slot.
    /// For each of [`crate::git::DATA_FILE`] and [`LABEL_FILE`]:
    /// `Some(bytes)` writes a new blob; `None` carries the prior
    /// HEAD tree's blob forward (or omits the slot entirely when
    /// there is no prior commit). The module's `refs/heads/main` is
    /// updated.
    ///
    /// Returns `Ok(None)` when the newly-built tree is byte-identical
    /// to the tree at the module's current HEAD. Git's
    /// content-addressing guarantees identical tree bytes produce
    /// identical oids, so a single oid equality check is sufficient.
    pub(crate) fn write_entry(
        &self,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<gix::ObjectId>, Error> {
        let br = self.bare();
        let prior_commit = br.read_head()?;
        // Only decode the prior tree when we actually need a blob from it.
        let prior_tree_entries = if (label.is_none() || data.is_none())
            && let Some(prior_id) = prior_commit
        {
            let prior = self.repo.find_object(prior_id)?.into_commit();
            let prior_tree_id = prior.decode()?.tree();
            Some(decode_tree(self.repo, prior_tree_id)?.entries)
        } else {
            None
        };
        let entries = build_slot_entries(self.repo, prior_tree_entries.as_deref(), label, data)?;
        let tree = Tree { entries };
        let tree_id = self.repo.write_object(&tree)?.detach();

        // No-op detection: if the module already has a HEAD whose
        // tree oid matches the tree we just built, every file (name,
        // mode, contents) is unchanged and we skip writing an
        // identical commit.
        if let Some(prior_id) = prior_commit {
            let prior = self.repo.find_object(prior_id)?.into_commit();
            let prior_tree_id = prior.decode()?.tree();
            if prior_tree_id == tree_id {
                return Ok(None);
            }
        }

        let commit_id = write_commit(
            self.repo,
            tree_id,
            prior_commit.into_iter().collect(),
            "put",
        )?;
        br.write_head(commit_id)?;
        Ok(Some(commit_id))
    }

    /// Read just the `label` blob at `commit`. Returns `None` for
    /// absent or empty label (the latter is how a commit explicitly
    /// clears its label slot). Used by the merge kernel to refresh
    /// the parent's label cache after a gitlink advance without
    /// round-tripping through [`Self::read_entry`].
    pub(crate) fn read_label(&self, commit: gix::ObjectId) -> Result<Option<Vec<u8>>, Error> {
        let tree_id = self
            .repo
            .find_object(commit)?
            .into_commit()
            .decode()?
            .tree();
        let tree = decode_tree(self.repo, tree_id)?;
        for entry in tree.entries {
            if entry.filename.as_bstr() == BStr::new(LABEL_FILE) {
                let blob = self.repo.find_object(entry.oid)?;
                if blob.data.is_empty() {
                    return Ok(None);
                }
                return Ok(Some(blob.data.clone()));
            }
        }
        Ok(None)
    }

    /// Build an [`Entry`] for the commit at `commit_id`. Reads the
    /// commit's time and its tree's `label` / `data` blobs; each slot
    /// is `Some(bytes)` if the corresponding file exists in the tree,
    /// `None` if absent. Tombstone commits (empty tree) surface as
    /// both slots `None`.
    pub(crate) fn read_entry(&self, commit_id: gix::ObjectId) -> Result<Entry, Error> {
        let tree_id = self
            .repo
            .find_object(commit_id)?
            .into_commit()
            .decode()?
            .tree();
        Entry::at_commit_tree(self.repo, commit_id, Some(tree_id))
    }
}
