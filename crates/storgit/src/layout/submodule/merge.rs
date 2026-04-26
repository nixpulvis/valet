//! Submodule-layout merge kernel.
//!
//! Walks the union of (base, local, incoming) gitlinks and decides
//! per id whether to advance, archive, conflict, or keep. Per-id
//! 3-way merges happen inside the submodule repos; the parent commit
//! chains so `gix::merge_base` can find the ancestor across syncs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use gix::bstr::{BStr, BString, ByteSlice};

use super::module::ModuleRepo;
use super::parent::ParentTree;
use super::{MODULES_DIR, PlannedOps, SubmoduleLayout, derive_module_url, ensure_module_repo};
use crate::{
    Distribute,
    error::Error,
    git::{BareRepo, read_ref_file, write_commit, write_merge_commit},
    id::EntryId,
    layout::Layout,
    merge::{
        BlobType, Conflict, FastForward, Merge, MergeProgress, MergeStatus, Outcome, Side,
        fold_conflict_blob_types, merge_tree_threeways,
    },
};

/// Build a parent-side gix index encoding the kernel's full decision
/// set. Every id that should remain live after the merge has a
/// stage-0 (`Unconflicted`) gitlink entry; conflicts are stored as
/// stages 1/2/3 (base/local/incoming). Archives are encoded as
/// absence: an id present in any of (base, local, incoming) but
/// without a stage-0 entry is archived. Persisted at
/// `<parent.git>/index` so the resolution path doesn't carry any
/// in-memory state across the conflict-resolve interval and `git
/// status` against the parent repo sees the conflict natively.
fn build_merge_index(
    object_hash: gix::hash::Kind,
    planned: &PlannedOps,
    conflicts: &[Conflict],
    local_gitlinks: &BTreeMap<EntryId, gix::ObjectId>,
    incoming_gitlinks: &BTreeMap<EntryId, gix::ObjectId>,
    base_gitlinks: &BTreeMap<EntryId, gix::ObjectId>,
) -> gix::index::State {
    let mut state = gix::index::State::new(object_hash);
    let conflict_ids: BTreeSet<&EntryId> = conflicts.iter().map(|c| &c.id).collect();

    // Stage 0: resolved planned advances and untouched local-only entries.
    for (id, oid) in &planned.advances {
        push_gitlink(&mut state, id, *oid, gix::index::entry::Stage::Unconflicted);
    }
    for (id, oid) in local_gitlinks {
        if conflict_ids.contains(id) || planned.archives.contains(id) {
            continue;
        }
        if planned.advances.contains_key(id) {
            continue;
        }
        // Local-only entry the kernel kept as-is.
        push_gitlink(&mut state, id, *oid, gix::index::entry::Stage::Unconflicted);
    }

    // Stages 1/2/3: per-id conflict, with whichever of base/local/incoming
    // exist for that id.
    for c in conflicts {
        if let Some(oid) = base_gitlinks.get(&c.id) {
            push_gitlink(&mut state, &c.id, *oid, gix::index::entry::Stage::Base);
        }
        if let Some(oid) = local_gitlinks.get(&c.id) {
            push_gitlink(&mut state, &c.id, *oid, gix::index::entry::Stage::Ours);
        }
        if let Some(oid) = incoming_gitlinks.get(&c.id) {
            push_gitlink(&mut state, &c.id, *oid, gix::index::entry::Stage::Theirs);
        }
    }
    state.sort_entries();
    state
}

fn push_gitlink(
    state: &mut gix::index::State,
    id: &EntryId,
    oid: gix::ObjectId,
    stage: gix::index::entry::Stage,
) {
    let flags = gix::index::entry::Flags::from_stage(stage);
    let path: BString = id.as_str().into();
    state.dangerously_push_entry(
        gix::index::entry::Stat::default(),
        oid,
        flags,
        gix::index::entry::Mode::COMMIT,
        path.as_bstr(),
    );
}

fn write_merge_index(parent_dir: &Path, state: gix::index::State) -> Result<(), Error> {
    let path = parent_dir.join("index");
    let mut file = gix::index::File::from_state(state, path);
    file.write(gix::index::write::Options::default())
        .map_err(|e| Error::Other(format!("write parent index: {e}")))?;
    Ok(())
}

fn clear_merge_index(parent_dir: &Path) -> Result<(), Error> {
    match std::fs::remove_file(parent_dir.join("index")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Read the parent merge index back, returning per-id stage maps the
/// resolution path consumes.
struct ResolvedIndex {
    /// Stage 0 entries: id -> resolved gitlink oid (the planned final
    /// state for non-conflicting ids).
    resolved: BTreeMap<EntryId, gix::ObjectId>,
    /// Conflicting ids, with their per-stage oids. Each `Option<oid>`
    /// is `None` when that side had no entry (e.g., delete/modify).
    conflicts: BTreeMap<EntryId, ConflictStages>,
}

#[derive(Default, Debug)]
struct ConflictStages {
    base: Option<gix::ObjectId>,
    local: Option<gix::ObjectId>,
    incoming: Option<gix::ObjectId>,
}

fn read_merge_index(parent_dir: &Path) -> Result<ResolvedIndex, Error> {
    let path = parent_dir.join("index");
    let file = gix::index::File::at(
        &path,
        crate::layout::HASH_TYPE,
        false,
        gix::index::decode::Options::default(),
    )
    .map_err(|e| Error::Other(format!("read parent index {path:?}: {e}")))?;
    let (state, _index_path) = file.into_parts();
    let path_backing = state.path_backing();
    let mut resolved: BTreeMap<EntryId, gix::ObjectId> = BTreeMap::new();
    let mut conflicts: BTreeMap<EntryId, ConflictStages> = BTreeMap::new();
    for entry in state.entries() {
        let bs: &BStr = entry.path_in(path_backing);
        let path_str = bs.to_str().map_err(|e| {
            Error::Other(format!("parent index: non-utf8 gitlink path {bs:?}: {e}"))
        })?;
        let id = EntryId::new(path_str.to_string())
            .map_err(|e| Error::Other(format!("parent index: bad gitlink path {bs:?}: {e}")))?;
        match entry.stage() {
            gix::index::entry::Stage::Unconflicted => {
                resolved.insert(id, entry.id);
            }
            gix::index::entry::Stage::Base => {
                conflicts.entry(id).or_default().base = Some(entry.id);
            }
            gix::index::entry::Stage::Ours => {
                conflicts.entry(id).or_default().local = Some(entry.id);
            }
            gix::index::entry::Stage::Theirs => {
                conflicts.entry(id).or_default().incoming = Some(entry.id);
            }
        }
    }
    Ok(ResolvedIndex {
        resolved,
        conflicts,
    })
}

impl SubmoduleLayout {
    pub(crate) fn run_merge_kernel(
        &mut self,
        incoming_parent: gix::ObjectId,
        incoming_gitlinks: BTreeMap<EntryId, gix::ObjectId>,
    ) -> Result<MergeStatus, Error> {
        let parent_path = self.parent_dir();
        BareRepo::new(&parent_path).ensure_no_merge_in_progress()?;

        let local_gitlinks: BTreeMap<EntryId, gix::ObjectId> = self.gitlinks().clone();

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
                        let module_path = self.module_dir(id);
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
            // Persist parent MERGE_HEAD plus a real gix index that
            // encodes the kernel's full decision set: stage-0 entries
            // for non-conflict planned moves and untouched local
            // gitlinks; stages 1/2/3 for conflicts. The resolution
            // path reads this back; nothing about the merge state
            // crosses the conflict-resolve interval in memory.
            let parent_br = BareRepo::new(&parent_path);
            parent_br.write_merge_head(incoming_parent)?;
            let state = build_merge_index(
                crate::layout::HASH_TYPE,
                &planned,
                &conflicts,
                &local_gitlinks,
                &incoming_gitlinks,
                &base_gitlinks,
            );
            write_merge_index(&parent_path, state)?;
            return Ok(MergeStatus::Conflicted(MergeProgress::new(conflicts)));
        }

        // Clean: apply planned ops, refresh label cache, flush parent.
        let forwards = self.apply_planned_ops(planned, incoming_parent)?;
        Ok(MergeStatus::Clean(forwards))
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
        let module_path = self.module_dir(id);
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
    ) -> Result<Vec<FastForward>, Error> {
        let mut forwards: Vec<FastForward> = Vec::new();
        for (id, oid) in planned.advances {
            let module = gix::open(self.module_dir(&id))?;
            let new_label = ModuleRepo::new(&module).read_label(oid)?;
            self.set_gitlink(id.clone(), oid);
            self.refresh_label_for(&id, new_label);
            forwards.push(FastForward {
                id: Some(id),
                commit: oid.into(),
            });
        }
        for id in planned.archives {
            <Self as Layout>::archive(self, &id)?;
        }
        self.flush_parent_with_extra_parent(Some(incoming_parent))?;
        Ok(forwards)
    }

    /// Materialise the parent tree, then chain a commit whose parents
    /// are the prior local HEAD plus optional `extra` (used to record
    /// the incoming side of a merge so subsequent merge-base lookups
    /// see both halves of history).
    fn flush_parent_with_extra_parent(
        &mut self,
        extra: Option<gix::ObjectId>,
    ) -> Result<(), Error> {
        self.flush_parent()?;
        let Some(extra) = extra else {
            return Ok(());
        };
        // After flush_parent the local HEAD is a one-parent commit
        // chained from the prior local HEAD. To make it a real merge
        // commit (two parents), rewrite it with `extra` appended.
        let parent_path = self.parent_dir();
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

    /// Fold `bundle`'s object graphs into this layout's parent.git
    /// and per-id module repos, creating any module repo that
    /// doesn't yet exist. Returns the incoming parent commit id (the
    /// parent tarball's `refs/heads/main`), or `None` if the parent
    /// tarball has no HEAD yet.
    ///
    /// Does not touch local refs, gitlinks, or merge state: after
    /// this returns, the incoming objects are reachable in the local
    /// object DB but nothing has forwards. Feed the returned commit
    /// id into [`SubmoduleLayout::run_merge_kernel`] to actually merge.
    pub(crate) fn import_bundle(
        &mut self,
        bundle: &crate::layout::submodule::Bundle,
    ) -> Result<Option<gix::ObjectId>, Error> {
        let incoming_parent =
            crate::tarball::import_tarball_objects(&bundle.parent, &self.parent_dir())?;
        for (id, bytes) in &bundle.modules {
            ensure_module_repo(self.root_path(), id)?;
            crate::tarball::import_tarball_objects(bytes, &self.module_dir(id))?;
        }
        Ok(incoming_parent)
    }
}

/// Walk the gitlink union; for every (Some(local), Some(incoming))
/// pair where local != incoming, verify local is an ancestor of
/// incoming. Returns the ids that fail the check.
pub(crate) fn preflight_ff_check(
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

impl Merge for SubmoduleLayout {
    fn merge_in_progress(&self) -> bool {
        BareRepo::new(&self.parent_dir()).merge_in_progress()
    }

    /// Abort an in-progress merge. Clears the parent's MERGE_HEAD
    /// and merge index, plus any per-submodule MERGE_HEAD files.
    fn abort(&mut self) -> Result<(), Error> {
        let root = self.root_path().to_path_buf();
        let parent_path = self.parent_dir();
        let parent_br = BareRepo::new(&parent_path);
        parent_br.clear_merge_head()?;
        clear_merge_index(&parent_path)?;
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

    /// Finalise an in-progress submodule merge using `resolution`.
    /// Applies the user's per-id picks plus every planned op the
    /// kernel had decided for non-conflicting ids; otherwise the
    /// non-picked half of the merge would be silently dropped.
    fn merge(&mut self, resolution: Outcome) -> Result<Vec<FastForward>, Error> {
        let parent_path = self.parent_dir();
        let parent_br = BareRepo::new(&parent_path);
        let incoming_parent = parent_br.require_merge_head("no merge in progress")?;

        // Snapshot the live gitlink set before any pick mutates it,
        // so the diff against `resolved` later actually reflects the
        // pre-merge state.
        let pre_merge: BTreeMap<EntryId, gix::ObjectId> = self.gitlinks().clone();

        // Read the merge index the kernel persisted: stage 0 entries
        // are non-conflict planned moves, stages 1/2/3 are the
        // per-conflict base/local/incoming gitlink oids. Archives are
        // ids that were live before but absent from the resolved set.
        let ResolvedIndex {
            mut resolved,
            conflicts,
        } = read_merge_index(&parent_path)?;

        // Resolve each conflict via the operator's pick. Run the
        // per-id submodule merge against the chosen side and add a
        // stage 0 entry (or leave absent for archived).
        let mut forwards: Vec<FastForward> = Vec::new();
        for (id, side) in resolution.picks.iter() {
            let stages = conflicts.get(id).ok_or_else(|| {
                Error::Other(format!("merge: id {id} not in merge index conflicts"))
            })?;
            let module_path = self.module_dir(id);
            let module_br = BareRepo::new(&module_path);
            let incoming_head_opt = stages.incoming;
            let local_head_opt = stages.local;
            // Per-submodule MERGE_HEAD is set whenever a per-id
            // 3-way merge was attempted (delete/modify or divergent
            // tree). Clear it; not all conflict shapes wrote one
            // (e.g. modify/delete with local kept) but clear_*
            // tolerates absence.
            module_br.clear_merge_head()?;
            match (local_head_opt, incoming_head_opt, *side) {
                // Both sides have the entry: per-id 3-way merge.
                (Some(local_head), Some(incoming_head), side) => {
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
                    let new_label = mr.read_label(chosen)?;
                    self.set_gitlink(id.clone(), merge_commit);
                    self.refresh_label_for(id, new_label);
                    forwards.push(FastForward {
                        id: Some(id.clone()),
                        commit: merge_commit.into(),
                    });
                    resolved.insert(id.clone(), merge_commit);
                }
                // delete/modify, operator adopts incoming: install the
                // incoming gitlink.
                (None, Some(incoming_head), Side::Incoming) => {
                    let module = gix::open(&module_path)?;
                    let new_label = ModuleRepo::new(&module).read_label(incoming_head)?;
                    self.set_gitlink(id.clone(), incoming_head);
                    self.refresh_label_for(id, new_label);
                    forwards.push(FastForward {
                        id: Some(id.clone()),
                        commit: incoming_head.into(),
                    });
                    resolved.insert(id.clone(), incoming_head);
                }
                // Operator chose archive: delete/modify with
                // Side::Local (keep local archive) or modify/delete
                // with Side::Incoming (adopt incoming archive).
                (None, Some(_), Side::Local) | (Some(_), None, Side::Incoming) => {
                    resolved.remove(id);
                }
                // modify/delete, operator keeps the local
                // modification: ensure stage 0 has the local oid so
                // flush_parent rebuilds with it.
                (Some(local_head), None, Side::Local) => {
                    resolved.insert(id.clone(), local_head);
                }
                // (None, None, _) can't appear: the kernel only
                // records a conflict when at least one of local /
                // incoming has the entry, and `conflicts.get(id)`
                // failed earlier for ids without any conflict stages.
                (None, None, _) => {
                    return Err(Error::Other(format!(
                        "merge: id {id} has no local or incoming stage"
                    )));
                }
            }
        }

        // Sync self.gitlinks to match `resolved`: any id present is
        // alive at that oid; any that was alive pre-merge but is
        // absent gets archived. The outer condition is true for
        // picks too (pre_merge was captured before the pick loop ran
        // set_gitlink); the inner `forwards` check is what keeps
        // this body from re-recording them. So in practice this
        // body only fires for non-conflict planned advances the
        // kernel persisted as stage-0 entries in the merge index.
        for (id, oid) in &resolved {
            if pre_merge.get(id).copied() != Some(*oid)
                && !forwards.iter().any(|a| a.id.as_ref() == Some(id))
            {
                // Non-conflict planned advance from the kernel.
                let module = gix::open(self.module_dir(id))?;
                let new_label = ModuleRepo::new(&module).read_label(*oid)?;
                self.set_gitlink(id.clone(), *oid);
                self.refresh_label_for(id, new_label);
                forwards.push(FastForward {
                    id: Some(id.clone()),
                    commit: (*oid).into(),
                });
            }
        }
        for id in pre_merge.keys() {
            if !resolved.contains_key(id) {
                <Self as Layout>::archive(self, id)?;
            }
        }

        // Chain a parent merge commit folding incoming_parent in so
        // future merges see both halves of history.
        //
        // Order is deliberate: flush first, then clear MERGE_HEAD and
        // the merge index. If the flush errors, both markers stay on
        // disk and the merge stays resumable; the operator can fix
        // whatever went wrong and call `merge` again, or `abort` to
        // discard. If we cleared first and the flush then errored,
        // the layout would look "no merge in progress" while the
        // parent ref hadn't actually forwards.
        self.flush_parent_with_extra_parent(Some(incoming_parent))?;
        parent_br.clear_merge_head()?;
        clear_merge_index(&parent_path)?;
        Ok(forwards)
    }
}

impl Distribute for SubmoduleLayout {
    /// Fetches the parent first, then for each gitlink whose oid
    /// differs from local, fetches that module's repo from the URL
    /// derived from the parent's remote URL. Then runs the merge
    /// kernel.
    fn pull(&mut self, remote: &str) -> Result<MergeStatus, Error> {
        // Fetch the parent first.
        self.fetch(remote)?;
        let parent_path = self.parent_dir();

        // Discover the incoming parent commit and its gitlinks.
        let tracking = parent_path.join("refs/remotes").join(remote).join("main");
        let Some(incoming_parent) = read_ref_file(&tracking)? else {
            return Ok(MergeStatus::Clean(Vec::new()));
        };
        let incoming_gitlinks = ParentTree::gitlinks_at(&parent_path, incoming_parent)?;

        // Look up the remote URL so we can derive per-module URLs.
        let remote_url = crate::config::GitConfig::lookup_remote(&parent_path, remote)?.url;
        for (id, incoming_oid) in &incoming_gitlinks {
            let local_oid = self.gitlinks().get(id).copied();
            if local_oid == Some(*incoming_oid) {
                continue;
            }
            ensure_module_repo(self.root_path(), id)?;
            let module_url = derive_module_url(&remote_url, id)?;
            crate::remote::fetch_into(&self.module_dir(id), &module_url)?;
        }

        self.run_merge_kernel(incoming_parent, incoming_gitlinks)
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
