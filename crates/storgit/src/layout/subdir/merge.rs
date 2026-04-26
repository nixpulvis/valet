//! Subdir-layout merge kernel.
//!
//! Merges two histories of the single shared ref by running
//! `gix::merge::tree` on their root trees. A clean auto-merge writes
//! the merge commit; a conflicted one stashes `MERGE_HEAD` so the
//! caller can drive a [`MergeProgress`] resolution.

use crate::{
    Distribute,
    error::Error,
    git::{BareRepo, decode_tree, read_ref_file, write_merge_commit},
    id::EntryId,
    layout::{
        Layout,
        subdir::{RecordsTree, SubdirLayout},
    },
    merge::{
        BlobType, Conflict, FastForward, Merge, MergeProgress, MergeStatus, Outcome, Side,
        fold_conflict_blob_types, merge_tree_threeways,
    },
};

/// Subtree name inside the repo's root tree that holds every entry.
const RECORDS_DIR: &str = "records";

impl SubdirLayout {
    /// Run the merge kernel. Merges `incoming_head` into the current
    /// `HEAD`. Returns [`MergeStatus::Clean`] when the merge is
    /// finalised and the ref advanced; [`MergeStatus::Conflicted`]
    /// when caller resolution is needed.
    pub(crate) fn run_merge_kernel(
        &mut self,
        incoming_head: gix::ObjectId,
    ) -> Result<MergeStatus, Error> {
        let git_dir = self.git_dir();
        let br = BareRepo::new(&git_dir);
        br.ensure_no_merge_in_progress()?;

        let repo = gix::open(&git_dir)?;
        let local_head = match br.read_head()? {
            Some(o) => o,
            None => {
                // Local empty: just point to incoming. This is a
                // load-from-nothing.
                br.write_head(incoming_head)?;
                self.rebuild_after_advance()?;
                return Ok(MergeStatus::Clean(vec![FastForward {
                    id: None,
                    commit: incoming_head.into(),
                }]));
            }
        };

        if local_head == incoming_head {
            return Ok(MergeStatus::Clean(Vec::new()));
        }

        // Treat "no common ancestor" (independent histories) as a
        // merge with the empty tree as the ancestor. This is common
        // when syncing two independently-initialised stores.
        let merge_base = match repo.merge_base(local_head, incoming_head) {
            Ok(id) => Some(id.detach()),
            Err(_) => None,
        };

        if let Some(base) = merge_base {
            if base == incoming_head {
                return Ok(MergeStatus::Clean(Vec::new()));
            }
            if base == local_head {
                br.write_head(incoming_head)?;
                self.rebuild_after_advance()?;
                return Ok(MergeStatus::Clean(vec![FastForward {
                    id: None,
                    commit: incoming_head.into(),
                }]));
            }
        }

        let mut outcome = merge_tree_threeways(&repo, merge_base, local_head, incoming_head)?;

        let how = gix::merge::tree::TreatAsUnresolved::git();
        if outcome.has_unresolved_conflicts(how) {
            let conflicts = build_conflicts(&outcome.conflicts, local_head, incoming_head, how);
            // Persist the auto-merged tree so resolution doesn't have
            // to re-run the three-way merge: it loads the saved tree
            // and overrides only the picked subtrees.
            let merge_tree_oid = outcome
                .tree
                .write()
                .map_err(|e| Error::Git(Box::new(e)))?
                .detach();
            br.write_merge_tree(merge_tree_oid)?;
            br.write_merge_head(incoming_head)?;
            return Ok(MergeStatus::Conflicted(MergeProgress::new(conflicts)));
        }

        // Auto-merged cleanly. Write merge tree, write merge commit.
        let merge_commit = write_merge_commit(
            &repo,
            outcome.tree,
            vec![local_head, incoming_head],
            &br.head_ref(),
        )?;
        self.rebuild_after_advance()?;
        Ok(MergeStatus::Clean(vec![FastForward {
            id: None,
            commit: merge_commit.into(),
        }]))
    }
}

impl Merge for SubdirLayout {
    fn merge_in_progress(&self) -> bool {
        BareRepo::new(&self.git_dir()).merge_in_progress()
    }

    fn abort(&mut self) -> Result<(), Error> {
        let git_dir = self.git_dir();
        let br = BareRepo::new(&git_dir);
        br.clear_merge_tree()?;
        br.clear_merge_head()
    }

    fn merge(&mut self, resolution: Outcome) -> Result<Vec<FastForward>, Error> {
        let git_dir = self.git_dir();
        let br = BareRepo::new(&git_dir);
        let incoming_head = br.require_merge_head("no merge in progress")?;
        let merge_tree_oid = br
            .read_merge_tree()?
            .ok_or_else(|| Error::Other("merge: MERGE_TREE missing".into()))?;
        let local_head = br
            .read_head()?
            .ok_or_else(|| Error::Other("merge: local HEAD missing".into()))?;
        let repo = gix::open(&git_dir)?;

        // Re-open the saved auto-merged tree as an editor and apply
        // pick overrides: for each picked id, swap the auto-merged
        // subtree (which carries conflict markers) with the chosen
        // side's records/<id>/ subtree.
        let merge_tree = repo.find_object(merge_tree_oid)?.into_tree();
        let mut editor = merge_tree.edit().map_err(|e| Error::Git(Box::new(e)))?;
        for (id, side) in resolution.picks.iter() {
            let chosen_root = match side {
                Side::Local => local_head,
                Side::Incoming => incoming_head,
            };
            let id_subtree_oid =
                RecordsTree::at_commit(&repo, Some(chosen_root))?.id_subtree(id)?;
            let id_path = format!("{RECORDS_DIR}/{}", id.as_str());
            // First clear any partially-resolved entries the
            // auto-merger left behind (conflict-marker blobs at
            // records/<id>/data, etc).
            editor
                .remove(id_path.as_str())
                .map_err(|e| Error::Git(Box::new(e)))?;
            // Then restore the chosen side's subtree blob-by-blob.
            if let Some(subtree_oid) = id_subtree_oid {
                let subtree = decode_tree(&repo, subtree_oid)?;
                for entry in subtree.entries {
                    let path = format!("{id_path}/{}", entry.filename);
                    editor
                        .upsert(path.as_str(), entry.mode.kind(), entry.oid)
                        .map_err(|e| Error::Git(Box::new(e)))?;
                }
            }
        }

        let merge_commit = write_merge_commit(
            &repo,
            editor,
            vec![local_head, incoming_head],
            &br.head_ref(),
        )?;
        br.clear_merge_head()?;
        br.clear_merge_tree()?;
        self.rebuild_after_advance()?;
        Ok(vec![FastForward {
            id: None,
            commit: merge_commit.into(),
        }])
    }

}

impl Distribute for SubdirLayout {
    fn pull(&mut self, remote: &str) -> Result<MergeStatus, Error> {
        self.fetch(remote)?;
        let tracking = self.git_dir().join("refs/remotes").join(remote).join("main");
        let Some(incoming) = read_ref_file(&tracking)? else {
            return Ok(MergeStatus::Clean(Vec::new()));
        };
        self.run_merge_kernel(incoming)
    }
}

/// Map gix `Conflict` entries to our [`Conflict`]. Aggregates
/// per-entry: a single id with conflicts on both data and label
/// surfaces as one [`Conflict`] with `BlobType::Both`.
fn build_conflicts(
    gix_conflicts: &[gix::merge::tree::Conflict],
    local_head: gix::ObjectId,
    incoming_head: gix::ObjectId,
    how: gix::merge::tree::TreatAsUnresolved,
) -> Vec<Conflict> {
    fold_conflict_blob_types(gix_conflicts, how, |path| {
        let (id, file) = split_record_path(path)?;
        let blob = BlobType::from_filename(file.as_str())?;
        Some((id, blob))
    })
    .into_iter()
    .map(|(id, blob)| Conflict {
        id,
        blob,
        local: local_head.into(),
        incoming: incoming_head.into(),
    })
    .collect()
}

fn split_record_path(path: &str) -> Option<(EntryId, String)> {
    let mut parts = path.splitn(3, '/');
    let first = parts.next()?;
    let id = parts.next()?;
    let file = parts.next()?;
    if first != RECORDS_DIR {
        return None;
    }
    let id = EntryId::new(id.to_string()).ok()?;
    Some((id, file.to_string()))
}
