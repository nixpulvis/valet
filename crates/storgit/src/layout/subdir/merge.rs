//! Subdir-layout merge kernel.
//!
//! Merges two histories of the single shared ref by running
//! `gix::merge::tree` on their root trees. A clean auto-merge writes
//! the merge commit; a conflicted one stashes `MERGE_HEAD` so the
//! caller can drive a [`MergeProgress`] resolution.

use crate::error::Error;
use crate::git::{BareRepo, decode_tree, read_ref_file, write_merge_commit};
use crate::id::CommitId;
use crate::id::EntryId;
use crate::layout::subdir::{RecordsTree, SubdirLayout};
use crate::merge::{
    BlobType, Conflict, MergeKernel, MergeProgress, MergeResolution, MergeStatus, Side,
    fold_conflict_blob_types, merge_tree_threeways,
};
use crate::store::Store;

/// Subtree name inside the repo's root tree that holds every entry.
const RECORDS_DIR: &str = "records";

impl Store<SubdirLayout> {
    /// Run the merge kernel. Merges `incoming_head` into the current
    /// `HEAD`. Returns [`MergeStatus::Clean`] when the merge is
    /// finalised and the ref advanced; [`MergeStatus::Conflicted`]
    /// when caller resolution is needed.
    pub(crate) fn run_merge_kernel(
        &mut self,
        incoming_head: gix::ObjectId,
    ) -> Result<MergeStatus<SubdirLayout>, Error> {
        let git_dir = self.git_dir();
        let br = BareRepo::new(&git_dir);
        if br.merge_in_progress() {
            return Err(Error::Other(
                "merge already in progress; resolve or abort first".into(),
            ));
        }

        let repo = gix::open(&git_dir)?;
        let local_head = match br.read_head()? {
            Some(o) => o,
            None => {
                // Local empty: just point to incoming. This is a
                // load-from-nothing.
                br.write_head(incoming_head)?;
                self.layout_mut_internal().rebuild_after_advance()?;
                return Ok(MergeStatus::Clean(vec![incoming_head.into()]));
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
                self.layout_mut_internal().rebuild_after_advance()?;
                return Ok(MergeStatus::Clean(vec![incoming_head.into()]));
            }
        }

        let outcome = merge_tree_threeways(&repo, merge_base, local_head, incoming_head)?;

        let how = gix::merge::tree::TreatAsUnresolved::git();
        if outcome.has_unresolved_conflicts(how) {
            let conflicts = build_conflicts(&outcome.conflicts, local_head, incoming_head, how);
            br.write_merge_head(incoming_head)?;
            return Ok(MergeStatus::Conflicted(MergeProgress::new(conflicts, ())));
        }

        // Auto-merged cleanly. Write merge tree, write merge commit.
        let merge_commit = write_merge_commit(
            &repo,
            outcome.tree,
            vec![local_head, incoming_head],
            &br.head_ref(),
        )?;
        self.layout_mut_internal().rebuild_after_advance()?;
        Ok(MergeStatus::Clean(vec![merge_commit.into()]))
    }

    fn layout_mut_internal(&mut self) -> &mut SubdirLayout {
        &mut self.layout
    }
}

impl MergeKernel for SubdirLayout {
    fn merge_in_progress(store: &Store<Self>) -> bool {
        BareRepo::new(&store.git_dir()).merge_in_progress()
    }

    fn abort(store: &mut Store<Self>) -> Result<(), Error> {
        BareRepo::new(&store.git_dir()).clear_merge_head()
    }

    fn merge(
        store: &mut Store<Self>,
        resolution: MergeResolution<SubdirLayout>,
    ) -> Result<Vec<CommitId>, Error> {
        let git_dir = store.git_dir();
        let br = BareRepo::new(&git_dir);
        let incoming_head = br.require_merge_head("no merge in progress")?;
        let local_head = br
            .read_head()?
            .ok_or_else(|| Error::Other("merge: local HEAD missing".into()))?;
        let repo = gix::open(&git_dir)?;

        // Build the merged tree from picks: for each picked id, take
        // that side's records/<id>/ subtree. Records not in the picks
        // come from the auto-merged base (we re-run the merge to get
        // it and then apply pick overrides).
        let merge_base = repo
            .merge_base(local_head, incoming_head)
            .ok()
            .map(|id| id.detach());
        let outcome = merge_tree_threeways(&repo, merge_base, local_head, incoming_head)?;

        // Walk the auto-merged tree editor, override each picked id
        // with the side chosen.
        let mut editor = outcome.tree;
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
        store.layout_mut_internal().rebuild_after_advance()?;
        Ok(vec![merge_commit.into()])
    }

    fn pull(store: &mut Store<Self>, remote: &str) -> Result<MergeStatus<SubdirLayout>, Error> {
        store.fetch(remote)?;
        let tracking = store
            .git_dir()
            .join("refs/remotes")
            .join(remote)
            .join("main");
        let Some(incoming) = read_ref_file(&tracking)? else {
            return Ok(MergeStatus::Clean(Vec::new()));
        };
        store.run_merge_kernel(incoming)
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
