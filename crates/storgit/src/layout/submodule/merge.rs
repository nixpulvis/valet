//! Submodule-layout merge kernel.
//!
//! Walks the union of (base, local, incoming) gitlinks and decides
//! per id whether to advance, archive, conflict, or keep. Per-id
//! 3-way merges happen inside the submodule repos; the parent commit
//! chains so `gix::merge_base` can find the ancestor across syncs.

use std::collections::{BTreeMap, HashMap};

use super::module::ModuleRepo;
use super::parent::ParentTree;
use super::{MODULES_DIR, PlannedOps, SubmoduleLayout, derive_module_url, ensure_module_repo};
use crate::error::Error;
use crate::git::{BareRepo, read_ref_file, write_commit, write_merge_commit};
use crate::id::CommitId;
use crate::id::EntryId;
use crate::layout::Layout;
use crate::merge::{
    ApplyMode, BlobType, Conflict, MergeKernel, MergeProgress, MergeResolution, MergeStatus, Side,
    fold_conflict_blob_types, merge_tree_threeways,
};
use crate::store::Store;

impl Store<SubmoduleLayout> {
    pub(crate) fn run_merge_kernel(
        &mut self,
        incoming_parent: gix::ObjectId,
        incoming_gitlinks: BTreeMap<EntryId, gix::ObjectId>,
    ) -> Result<MergeStatus<SubmoduleLayout>, Error> {
        if self.merge_in_progress() {
            return Err(Error::Other(
                "merge already in progress; resolve or abort first".into(),
            ));
        }

        let parent_path = self.layout.parent_path();
        let local_gitlinks: BTreeMap<EntryId, gix::ObjectId> = self.layout.gitlinks().clone();

        // Resolve the parent's three-way base. Without it, archives and
        // adds are indistinguishable on the (None, Some) / (Some, None)
        // sides. With chained parent commits, fetch has brought
        // incoming history into the local parent's object DB so
        // merge_base can walk it.
        let local_parent_head = BareRepo::new(&parent_path).read_head()?;
        let base_gitlinks: BTreeMap<EntryId, gix::ObjectId> = match local_parent_head {
            Some(local_head) => {
                let parent_repo = gix::open(&parent_path)?;
                match parent_repo.merge_base(local_head, incoming_parent) {
                    Ok(base) => ParentTree::gitlinks_at(&parent_path, base.detach())?,
                    Err(_) => BTreeMap::new(),
                }
            }
            None => BTreeMap::new(),
        };

        let mut conflicts: Vec<Conflict> = Vec::new();
        let mut planned = PlannedOps::default();

        let all_ids: std::collections::BTreeSet<&EntryId> = local_gitlinks
            .keys()
            .chain(incoming_gitlinks.keys())
            .chain(base_gitlinks.keys())
            .collect();

        for id in all_ids {
            let local = local_gitlinks.get(id).copied();
            let incoming = incoming_gitlinks.get(id).copied();
            let base = base_gitlinks.get(id).copied();
            match (base, local, incoming) {
                (_, Some(a), Some(b)) if a == b => {}
                (_, None, None) => {}

                // Independently or jointly added/modified on both sides.
                (_, Some(a), Some(b)) => {
                    self.merge_module_pair(id, a, b, &mut planned, &mut conflicts)?;
                }

                // Incoming-only id.
                (None, None, Some(b)) => {
                    // Added on incoming since the merge base; adopt.
                    planned.advances.insert(id.clone(), b);
                }
                (Some(b_oid), None, Some(i_oid)) => {
                    if b_oid == i_oid {
                        // Incoming unchanged since base; local archive
                        // wins. No-op.
                    } else {
                        // Incoming modified an entry the local side
                        // archived: classic delete-vs-modify. Surface
                        // as conflict so the operator chooses.
                        let module_path = self.layout.module_path(id);
                        BareRepo::new(&module_path).write_merge_head(i_oid)?;
                        conflicts.push(Conflict {
                            id: id.clone(),
                            blob: BlobType::Both,
                            local: b_oid.into(),
                            incoming: i_oid.into(),
                        });
                    }
                }

                // Local-only id (incoming archived or never had it).
                (None, Some(_), None) => {
                    // Added on local since the merge base; keep.
                }
                (Some(b_oid), Some(a_oid), None) => {
                    if b_oid == a_oid {
                        // Local unchanged since base; incoming archived
                        // it. Propagate the archive.
                        planned.archives.push(id.clone());
                    } else {
                        // Local modified since base; incoming archived.
                        // Modify-vs-archive conflict. Keep local; a
                        // future enhancement could surface a richer
                        // conflict here.
                    }
                }
            }
        }

        if !conflicts.is_empty() {
            // Persist parent MERGE_HEAD; parent ref unchanged until
            // resolution commits.
            BareRepo::new(&parent_path).write_merge_head(incoming_parent)?;
            return Ok(MergeStatus::Conflicted(MergeProgress::new(
                conflicts, planned,
            )));
        }

        // Clean: apply planned ops, refresh label cache, flush parent.
        let advanced = self.apply_planned_ops(planned, incoming_parent)?;
        Ok(MergeStatus::Clean(advanced))
    }

    /// Run a 3-way merge inside the submodule for `id`. On clean
    /// auto-merge, writes the merge commit and records a planned
    /// advance. On conflict, writes the submodule's MERGE_HEAD and
    /// pushes a Conflict.
    fn merge_module_pair(
        &mut self,
        id: &EntryId,
        a: gix::ObjectId,
        b: gix::ObjectId,
        planned: &mut PlannedOps,
        conflicts: &mut Vec<Conflict>,
    ) -> Result<(), Error> {
        let module_path = self.layout.module_path(id);
        let br = BareRepo::new(&module_path);
        let module = gix::open(&module_path)?;
        let merge_base = module.merge_base(a, b).ok().map(|i| i.detach());
        if merge_base == Some(a) {
            planned.advances.insert(id.clone(), b);
            return Ok(());
        }
        if merge_base == Some(b) {
            return Ok(());
        }
        let outcome = merge_tree_threeways(&module, merge_base, a, b)?;
        let how = gix::merge::tree::TreatAsUnresolved::git();
        if outcome.has_unresolved_conflicts(how) {
            conflicts.push(Conflict {
                id: id.clone(),
                blob: classify_module_conflicts(&outcome.conflicts, how),
                local: a.into(),
                incoming: b.into(),
            });
            br.write_merge_head(b)?;
        } else {
            let mc = write_merge_commit(&module, outcome.tree, vec![a, b], &br.head_ref())?;
            planned.advances.insert(id.clone(), mc);
        }
        Ok(())
    }

    /// Apply a planned-ops batch (used by clean merges and by the
    /// resolution path after picks have been applied). Folds
    /// `incoming_parent` into the parent commit history so future
    /// merges can find the right base.
    fn apply_planned_ops(
        &mut self,
        planned: PlannedOps,
        incoming_parent: gix::ObjectId,
    ) -> Result<HashMap<EntryId, CommitId>, Error> {
        let mut advanced: HashMap<EntryId, CommitId> = HashMap::new();
        for (id, oid) in planned.advances {
            let module = gix::open(self.layout.module_path(&id))?;
            let new_label = ModuleRepo::new(&module).read_label(oid)?;
            self.layout_mut().set_gitlink(id.clone(), oid);
            self.layout_mut().refresh_label_for(&id, new_label);
            advanced.insert(id, oid.into());
        }
        for id in planned.archives {
            self.layout_mut().archive(&id)?;
        }
        self.flush_parent_with_extra_parent(Some(incoming_parent))?;
        Ok(advanced)
    }

    /// Materialise the parent tree, then chain a commit whose parents
    /// are the prior local HEAD plus optional `extra` (used to record
    /// the incoming side of a merge so subsequent merge-base lookups
    /// see both halves of history).
    fn flush_parent_with_extra_parent(
        &mut self,
        extra: Option<gix::ObjectId>,
    ) -> Result<(), Error> {
        let layout = &mut self.layout;
        layout.flush_parent_pub()?;
        let Some(extra) = extra else {
            return Ok(());
        };
        // After flush_parent the local HEAD is a one-parent commit
        // chained from the prior local HEAD. To make it a real merge
        // commit (two parents), rewrite it with `extra` appended.
        let parent_path = layout.parent_path();
        let br = BareRepo::new(&parent_path);
        let head = match br.read_head()? {
            Some(h) => h,
            None => return Ok(()),
        };
        let parent_repo = gix::open(&parent_path)?;
        let head_commit = parent_repo.find_object(head)?.into_commit();
        let decoded = head_commit.decode()?;
        let tree_oid = decoded.tree();
        let mut new_parents: Vec<gix::ObjectId> = decoded.parents().collect();
        if new_parents.iter().any(|p| *p == extra) {
            return Ok(());
        }
        new_parents.push(extra);
        let new_commit = write_commit(&parent_repo, tree_oid, new_parents, "merge")?;
        br.write_head(new_commit)?;
        Ok(())
    }

    /// Apply `parts` onto this store with default merge semantics.
    /// See [`Store::apply_with`] for the full-control variant.
    pub fn apply(
        &mut self,
        parts: crate::layout::submodule::Parts,
    ) -> Result<MergeStatus<SubmoduleLayout>, Error> {
        self.apply_with(parts, ApplyMode::Merge)
    }

    /// Apply `parts` with explicit [`ApplyMode`].
    ///
    /// - [`ApplyMode::Merge`]: client-side default. Merges via the
    ///   kernel; returns [`MergeStatus::Conflicted`] when human
    ///   resolution is needed.
    /// - [`ApplyMode::FastForwardOnly`]: server-side accept-push.
    ///   Errors with [`Error::NotFastForward`] if any gitlink
    ///   change is divergent. Caller's recovery is to pull and
    ///   merge locally, then resend.
    pub fn apply_with(
        &mut self,
        parts: crate::layout::submodule::Parts,
        mode: ApplyMode,
    ) -> Result<MergeStatus<SubmoduleLayout>, Error> {
        if parts.parent.is_empty() {
            stash_pending_modules(&mut self.layout, parts);
            return Ok(MergeStatus::Clean(HashMap::new()));
        }

        let Some(incoming_parent) = self.import_parts(&parts)? else {
            return Ok(MergeStatus::Clean(HashMap::new()));
        };
        let parent_path = self.layout.parent_path();
        let incoming_gitlinks = ParentTree::gitlinks_at(&parent_path, incoming_parent)?;

        if mode == ApplyMode::FastForwardOnly {
            let diverging = preflight_ff_check(self.layout.gitlinks(), &incoming_gitlinks, |id| {
                self.layout.module_path(id)
            })?;
            if !diverging.is_empty() {
                return Err(Error::NotFastForward { ids: diverging });
            }
        }

        self.run_merge_kernel(incoming_parent, incoming_gitlinks)
    }

    /// Fold `parts`' object graphs into this store's parent.git and
    /// per-id module repos, creating any module repo that doesn't
    /// yet exist. Returns the incoming parent commit id (the parent
    /// tarball's `refs/heads/main`), or `None` if the parent tarball
    /// has no HEAD yet.
    ///
    /// Does not touch local refs, gitlinks, or merge state: after
    /// this returns, the incoming objects are reachable in the local
    /// object DB but nothing has advanced. Feed the returned commit
    /// id into [`Store::run_merge_kernel`] to actually merge.
    pub(crate) fn import_parts(
        &mut self,
        parts: &crate::layout::submodule::Parts,
    ) -> Result<Option<gix::ObjectId>, Error> {
        let incoming_parent =
            crate::tarball::import_tarball_objects(&parts.parent, &self.layout.parent_path())?;
        for (id, bytes) in &parts.modules {
            ensure_module_repo(self.layout.root_path(), id)?;
            crate::tarball::import_tarball_objects(bytes, &self.layout.module_path(id))?;
        }
        Ok(incoming_parent)
    }

    fn layout_mut(&mut self) -> &mut SubmoduleLayout {
        &mut self.layout
    }
}

// --- Helpers for `apply` / `apply_with` ----------------------------

fn stash_pending_modules(layout: &mut SubmoduleLayout, parts: crate::layout::submodule::Parts) {
    let pending = layout.pending_modules_mut();
    for (id, bytes) in parts.modules {
        pending.insert(id, bytes);
    }
}

/// Walk the gitlink union; for every (Some(local), Some(incoming))
/// pair where local != incoming, verify local is an ancestor of
/// incoming. Returns the ids that fail the check.
fn preflight_ff_check(
    local: &BTreeMap<EntryId, gix::ObjectId>,
    incoming: &BTreeMap<EntryId, gix::ObjectId>,
    module_path: impl Fn(&EntryId) -> std::path::PathBuf,
) -> Result<Vec<String>, Error> {
    let mut diverging = Vec::new();
    for (id, incoming_oid) in incoming {
        let Some(&local_oid) = local.get(id) else {
            continue;
        };
        if local_oid == *incoming_oid {
            continue;
        }
        let module = gix::open(module_path(id))?;
        let merge_base = module
            .merge_base(local_oid, *incoming_oid)
            .ok()
            .map(|i| i.detach());
        if merge_base != Some(local_oid) {
            diverging.push(id.to_string());
        }
    }
    Ok(diverging)
}

impl MergeKernel for SubmoduleLayout {
    fn merge_in_progress(store: &Store<Self>) -> bool {
        BareRepo::new(&store.layout.parent_path()).merge_in_progress()
    }

    /// Abort an in-progress merge. Clears the parent's MERGE_HEAD
    /// and any per-submodule MERGE_HEAD files.
    fn abort(store: &mut Store<Self>) -> Result<(), Error> {
        let root = store.layout.root_path().to_path_buf();
        BareRepo::new(&store.layout.parent_path()).clear_merge_head()?;
        // Clear per-submodule MERGE_HEAD files. We only know about
        // ids in the current gitlinks, but a conflict-side gitlink
        // may belong to an id absent from local state. Walk the
        // modules dir and clear MERGE_HEAD anywhere it exists.
        let modules_dir = root.join(MODULES_DIR);
        if let Ok(read_dir) = std::fs::read_dir(&modules_dir) {
            for entry in read_dir.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    BareRepo::new(&p).clear_merge_head()?;
                }
            }
        }
        Ok(())
    }

    /// Fetch from `remote` (parent + every module whose gitlink
    /// changed) and merge.
    fn pull(store: &mut Store<Self>, remote: &str) -> Result<MergeStatus<SubmoduleLayout>, Error> {
        // Fetch the parent first.
        store.fetch(remote)?;

        // Discover the incoming parent commit and its gitlinks.
        let parent_path = store.layout.parent_path();
        let tracking = parent_path.join("refs/remotes").join(remote).join("main");
        let Some(incoming_parent) = read_ref_file(&tracking)? else {
            return Ok(MergeStatus::Clean(HashMap::new()));
        };
        let incoming_gitlinks = ParentTree::gitlinks_at(&parent_path, incoming_parent)?;

        // Look up the remote URL so we can derive per-module URLs.
        let remote_url = crate::remote::Remotes::new(&parent_path).lookup_url(remote)?;
        for (id, incoming_oid) in &incoming_gitlinks {
            let local_oid = store.layout.gitlinks().get(id).copied();
            if local_oid == Some(*incoming_oid) {
                continue;
            }
            ensure_module_repo(store.layout.root_path(), id)?;
            let module_url = derive_module_url(&remote_url, id)?;
            crate::remote::fetch_into(&store.layout.module_path(id), &module_url)?;
        }

        store.run_merge_kernel(incoming_parent, incoming_gitlinks)
    }

    /// Finalise an in-progress submodule merge using `resolution`.
    /// Applies the user's per-id picks plus every planned op the
    /// kernel had decided for non-conflicting ids; otherwise the
    /// non-picked half of the merge would be silently dropped.
    fn merge(
        store: &mut Store<Self>,
        resolution: MergeResolution<SubmoduleLayout>,
    ) -> Result<HashMap<EntryId, CommitId>, Error> {
        let parent_path = store.layout.parent_path();
        let parent_br = BareRepo::new(&parent_path);
        let incoming_parent = parent_br.require_merge_head("no merge in progress")?;

        let mut advanced: HashMap<EntryId, CommitId> = HashMap::new();
        for (id, side) in resolution.picks.iter() {
            let module_path = store.layout.module_path(id);
            let module_br = BareRepo::new(&module_path);
            let incoming_head =
                module_br.require_merge_head(&format!("missing MERGE_HEAD for {id}"))?;
            let local_head = store.layout.gitlinks().get(id).copied();

            match (local_head, *side) {
                (Some(local_head), side) => {
                    let module = gix::open(&module_path)?;
                    let mr = ModuleRepo::new(&module);
                    let chosen = match side {
                        Side::Local => local_head,
                        Side::Incoming => incoming_head,
                    };
                    let chosen_tree = module.find_object(chosen)?.into_commit().decode()?.tree();
                    let merge_commit = write_commit(
                        &module,
                        chosen_tree,
                        vec![local_head, incoming_head],
                        "merge",
                    )?;
                    module_br.write_head(merge_commit)?;
                    module_br.clear_merge_head()?;

                    let new_label = mr.read_label(chosen)?;
                    store.layout_mut().set_gitlink(id.clone(), merge_commit);
                    store.layout_mut().refresh_label_for(id, new_label);
                    advanced.insert(id.clone(), merge_commit.into());
                }
                (None, Side::Local) => {
                    // Local archived; pick local means stay archived.
                    module_br.clear_merge_head()?;
                }
                (None, Side::Incoming) => {
                    // Local archived but operator picked incoming:
                    // adopt the incoming oid.
                    module_br.clear_merge_head()?;
                    let module = gix::open(&module_path)?;
                    let new_label = ModuleRepo::new(&module).read_label(incoming_head)?;
                    store.layout_mut().set_gitlink(id.clone(), incoming_head);
                    store.layout_mut().refresh_label_for(id, new_label);
                    advanced.insert(id.clone(), incoming_head.into());
                }
            }
        }

        // Apply non-conflicting planned ops. Each side advance and
        // each planned archive needs to land before we close the
        // merge, otherwise non-picked changes get silently dropped.
        parent_br.clear_merge_head()?;
        advanced.extend(store.apply_planned_ops(resolution.planned, incoming_parent)?);
        Ok(advanced)
    }
}

fn classify_module_conflicts(
    gix_conflicts: &[gix::merge::tree::Conflict],
    how: gix::merge::tree::TreatAsUnresolved,
) -> BlobType {
    let folded = fold_conflict_blob_types(gix_conflicts, how, |path| {
        BlobType::from_filename(path).map(|b| ((), b))
    });
    // Empty map (no recognised slot files) is pessimistic: report
    // Both so the operator sees there's something unresolved.
    folded.get(&()).copied().unwrap_or(BlobType::Both)
}
