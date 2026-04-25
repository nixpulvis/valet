//! Shared merge types used by every layout.
//!
//! The merge kernels live next to each layout (see
//! [`crate::layout::subdir`] and [`crate::layout::submodule`]); this
//! module defines the cross-cutting vocabulary they speak.
//!
//! Both kernels drive [`apply`] and [`pull`] and return a
//! [`MergeStatus`]. A `Clean` status means the merge is already
//! finalised. A `Conflicted` status carries a [`MergeProgress`]
//! the caller drives: inspect [`MergeProgress::conflicts`], call
//! [`MergeProgress::pick`] for each, then [`MergeProgress::resolve`]
//! to produce a [`MergeResolution`] that [`Store::merge`] accepts.
//!
//! [`apply`]: crate::Store::apply
//! [`pull`]: crate::Store::pull
//! [`Store::merge`]: crate::Store::merge

use std::collections::HashMap;
use std::marker::PhantomData;

use crate::error::Error;
use crate::id::CommitId;
use crate::id::EntryId;
use crate::layout::Layout;

/// Walk a gix conflict list, keeping only unresolved entries, and
/// fold each surviving entry's extracted key/blob pair into a map,
/// collapsing repeated keys via [`BlobType::combine`]. `key` receives
/// the conflict's path location and returns `Some((key, blob))` to
/// include the conflict or `None` to drop it.
///
/// Used by both layouts' conflict classifiers: subdir keys by
/// [`EntryId`] (one bucket per record), submodule keys by `()` (one
/// bucket for the whole module).
pub(crate) fn fold_conflict_blob_types<K, F>(
    gix_conflicts: &[gix::merge::tree::Conflict],
    how: gix::merge::tree::TreatAsUnresolved,
    mut key: F,
) -> HashMap<K, BlobType>
where
    K: Eq + std::hash::Hash,
    F: FnMut(&str) -> Option<(K, BlobType)>,
{
    let mut out: HashMap<K, BlobType> = HashMap::new();
    for c in gix_conflicts {
        if !c.is_unresolved(how) {
            continue;
        }
        let Some(path) = location_from_conflict(c) else {
            continue;
        };
        let Some((k, blob)) = key(&path) else {
            continue;
        };
        out.entry(k)
            .and_modify(|b| *b = b.combine(blob))
            .or_insert(blob);
    }
    out
}

/// Extract the path location from a gix merge conflict, regardless of
/// which `Change` variant the `ours` side is. Bridges two foreign
/// types so it has no natural home on either; stays a free helper.
pub(crate) fn location_from_conflict(c: &gix::merge::tree::Conflict) -> Option<String> {
    use gix::diff::tree_with_rewrites::Change;
    let location = match &c.ours {
        Change::Addition { location, .. } => location,
        Change::Deletion { location, .. } => location,
        Change::Modification { location, .. } => location,
        Change::Rewrite {
            source_location, ..
        } => source_location,
    };
    Some(location.to_string())
}

/// Run `gix::merge_trees` on the three commit trees with storgit's
/// canonical labels. Falls back to an empty tree as the ancestor when
/// `merge_base` is `None`.
pub(crate) fn merge_tree_threeways<'a>(
    repo: &'a gix::Repository,
    merge_base: Option<gix::ObjectId>,
    ours: gix::ObjectId,
    theirs: gix::ObjectId,
) -> Result<gix::merge::tree::Outcome<'a>, Error> {
    let our_tree = repo.find_object(ours)?.into_commit().decode()?.tree();
    let their_tree = repo.find_object(theirs)?.into_commit().decode()?.tree();
    let ancestor_tree = match merge_base {
        Some(b) => repo.find_object(b)?.into_commit().decode()?.tree(),
        None => gix::ObjectId::empty_tree(repo.object_hash()),
    };
    let opts = repo
        .tree_merge_options()
        .map_err(|e| Error::Git(Box::new(e)))?;
    let labels = gix::merge::blob::builtin_driver::text::Labels {
        ancestor: Some("base".into()),
        current: Some("ours".into()),
        other: Some("theirs".into()),
    };
    repo.merge_trees(ancestor_tree, our_tree, their_tree, labels, opts)
        .map_err(|e| Error::Git(Box::new(e)))
}

/// Which side of a merge wins a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Local,
    Incoming,
}

/// How aggressive `apply` is allowed to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    /// Allow 3-way merges; surface conflicts via [`MergeProgress`]
    /// for human resolution. The default for client-side use.
    Merge,
    /// Reject anything that isn't a fast-forward. Used by
    /// server-side accept-push handlers that have no operator
    /// to resolve conflicts; the client is expected to pull and
    /// merge locally, then resend.
    FastForwardOnly,
}

impl Default for ApplyMode {
    fn default() -> Self {
        ApplyMode::Merge
    }
}

/// Which of an entry's blobs diverged. Informational only:
/// [`MergeProgress::pick`] always resolves the whole entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlobType {
    Data,
    Label,
    Both,
}

impl BlobType {
    /// Classify a per-entry filename. Returns `None` for filenames
    /// that aren't part of an entry's blob set.
    pub(crate) fn from_filename(file: &str) -> Option<Self> {
        match file {
            "data" => Some(BlobType::Data),
            "label" => Some(BlobType::Label),
            _ => None,
        }
    }

    /// Fold another observation into this one. Different blob types
    /// collapse to [`BlobType::Both`].
    pub(crate) fn combine(self, other: Self) -> Self {
        if self != other { BlobType::Both } else { self }
    }
}

/// A per-entry conflict surfaced by the merge kernel.
#[derive(Debug, Clone)]
pub struct Conflict {
    pub id: EntryId,
    pub blob: BlobType,
    /// Commit on the local side exposing the conflicting blob.
    /// For submodule layout this is the submodule's HEAD; for
    /// subdir it is the store HEAD (identical across every
    /// `Conflict` in a single merge).
    pub local: CommitId,
    /// Incoming side, same interpretation as `local`.
    pub incoming: CommitId,
}

impl Conflict {
    /// Commit for the requested side.
    pub fn commit(&self, side: Side) -> CommitId {
        match side {
            Side::Local => self.local.clone(),
            Side::Incoming => self.incoming.clone(),
        }
    }
}

/// Outcome of [`Store::apply`] or [`Store::pull`].
///
/// [`Store::apply`]: crate::Store::apply
/// [`Store::pull`]: crate::Store::pull
#[derive(Debug)]
pub enum MergeStatus<L: Layout> {
    /// Merge finalised; `Advanced` reports what moved in the
    /// local store. May be empty for a no-op apply.
    Clean(L::Advanced),
    /// Merge paused on conflicts. Drive the progress through
    /// `pick` + `resolve`, then call [`Store::merge`].
    ///
    /// [`Store::merge`]: crate::Store::merge
    Conflicted(MergeProgress<L>),
}

/// In-progress merge state: the conflicts surfaced by the kernel
/// plus the caller's picks so far. Also carries any non-conflicting
/// ops the kernel had already decided ([`Layout::PlannedOps`]) so the
/// resolution path can apply them alongside the picks.
#[derive(Debug)]
pub struct MergeProgress<L: Layout> {
    conflicts: Vec<Conflict>,
    picks: HashMap<EntryId, Side>,
    pub(crate) planned: L::PlannedOps,
    _layout: PhantomData<fn() -> L>,
}

impl<L: Layout> MergeProgress<L> {
    pub(crate) fn new(conflicts: Vec<Conflict>, planned: L::PlannedOps) -> Self {
        Self {
            conflicts,
            picks: HashMap::new(),
            planned,
            _layout: PhantomData,
        }
    }

    /// All conflicts the kernel surfaced. Stable; does not shrink
    /// as picks are recorded.
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    /// Conflicts without a pick recorded yet.
    pub fn remaining(&self) -> Vec<&Conflict> {
        self.conflicts
            .iter()
            .filter(|c| !self.picks.contains_key(&c.id))
            .collect()
    }

    /// Record which side wins for one conflict. A later `pick`
    /// for the same id overwrites the prior choice. Errors if
    /// `id` isn't in [`conflicts`](Self::conflicts).
    pub fn pick(&mut self, id: EntryId, side: Side) -> Result<(), Error> {
        if !self.conflicts.iter().any(|c| c.id == id) {
            return Err(Error::Other(format!("pick: id {id} is not in conflicts")));
        }
        self.picks.insert(id, side);
        Ok(())
    }

    /// Validate and consume. Errors if any conflict is unpicked.
    pub fn resolve(self) -> Result<MergeResolution<L>, Error> {
        let remaining: Vec<String> = self
            .conflicts
            .iter()
            .filter(|c| !self.picks.contains_key(&c.id))
            .map(|c| c.id.to_string())
            .collect();
        if !remaining.is_empty() {
            return Err(Error::Other(format!(
                "resolve: unpicked conflicts: {remaining:?}"
            )));
        }
        Ok(MergeResolution {
            picks: self.picks,
            planned: self.planned,
            _layout: PhantomData,
        })
    }
}

/// Validated merge resolution. The only constructor is
/// [`MergeProgress::resolve`]; the only consumer is
/// [`Store::merge`](crate::Store::merge).
#[derive(Debug)]
pub struct MergeResolution<L: Layout> {
    pub(crate) picks: HashMap<EntryId, Side>,
    pub(crate) planned: L::PlannedOps,
    _layout: PhantomData<fn() -> L>,
}

impl<L: Layout> MergeResolution<L> {
    /// The pick recorded for `id`, if any.
    pub fn pick_for(&self, id: &EntryId) -> Option<Side> {
        self.picks.get(id).copied()
    }
}

/// The common `apply` / `pull` / `merge` / `abort` surface both
/// layouts expose on top of [`Layout`]. Per-layout implementations
/// live next to their merge kernels; `Store<L>` has generic
/// delegating wrappers over this trait so callers write
/// `store.pull(...)` without caring which layout `L` is.
///
/// Signatures take `&Store<Self>` (or `&mut`) rather than `&self` on
/// the layout because the merge kernels need the whole store (for
/// `fetch`, for `Store::apply_planned_ops`, and so on), not just
/// the layout's on-disk state.
pub trait MergeKernel: Layout {
    /// True when a merge is in progress and the operator still needs
    /// to call [`Self::merge`] or [`Self::abort`].
    fn merge_in_progress(store: &crate::store::Store<Self>) -> bool;

    /// Abort an in-progress merge, clearing any `MERGE_HEAD` markers.
    /// No-op when no merge is in progress.
    fn abort(store: &mut crate::store::Store<Self>) -> Result<(), Error>;

    /// Finalise an in-progress merge using the picks in `resolution`.
    fn merge(
        store: &mut crate::store::Store<Self>,
        resolution: MergeResolution<Self>,
    ) -> Result<Self::Advanced, Error>;

    /// Fetch from `remote` and merge its branch into the local
    /// store. A remote with no branch yet is a clean no-op.
    fn pull(
        store: &mut crate::store::Store<Self>,
        remote: &str,
    ) -> Result<MergeStatus<Self>, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::submodule::SubmoduleLayout;

    fn commit(b: u8) -> CommitId {
        let raw = gix::ObjectId::from_bytes_or_panic(&[b; 20]);
        CommitId::from(raw)
    }

    fn conflict(id: &str, blob: BlobType, local: u8, incoming: u8) -> Conflict {
        Conflict {
            id: EntryId::new(id.to_string()).unwrap(),
            blob,
            local: commit(local),
            incoming: commit(incoming),
        }
    }

    #[test]
    fn conflict_commit_by_side() {
        let c = conflict("alpha", BlobType::Data, 0xaa, 0xbb);
        assert_eq!(c.commit(Side::Local), commit(0xaa));
        assert_eq!(c.commit(Side::Incoming), commit(0xbb));
    }

    fn progress(conflicts: Vec<Conflict>) -> MergeProgress<SubmoduleLayout> {
        MergeProgress::new(conflicts, Default::default())
    }

    #[test]
    fn pick_unknown_id_errors() {
        let mut p = progress(vec![conflict("alpha", BlobType::Data, 1, 2)]);
        let other = EntryId::new("beta".to_string()).unwrap();
        assert!(p.pick(other, Side::Local).is_err());
    }

    #[test]
    fn resolve_partial_errors() {
        let mut p = progress(vec![
            conflict("alpha", BlobType::Data, 1, 2),
            conflict("beta", BlobType::Label, 3, 4),
        ]);
        p.pick(EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        assert!(p.resolve().is_err());
    }

    #[test]
    fn resolve_full_success() {
        let mut p = progress(vec![
            conflict("alpha", BlobType::Data, 1, 2),
            conflict("beta", BlobType::Label, 3, 4),
        ]);
        p.pick(EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        p.pick(EntryId::new("beta".to_string()).unwrap(), Side::Incoming)
            .unwrap();
        let r = p.resolve().unwrap();
        assert_eq!(
            r.pick_for(&EntryId::new("alpha".to_string()).unwrap()),
            Some(Side::Local)
        );
        assert_eq!(
            r.pick_for(&EntryId::new("beta".to_string()).unwrap()),
            Some(Side::Incoming)
        );
    }

    #[test]
    fn remaining_shrinks_with_picks() {
        let mut p = progress(vec![
            conflict("alpha", BlobType::Data, 1, 2),
            conflict("beta", BlobType::Label, 3, 4),
        ]);
        assert_eq!(p.remaining().len(), 2);
        p.pick(EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        assert_eq!(p.remaining().len(), 1);
        assert_eq!(p.remaining()[0].id.as_str(), "beta");
    }

    #[test]
    fn conflicts_is_stable_under_picks() {
        let mut p = progress(vec![conflict("alpha", BlobType::Data, 1, 2)]);
        assert_eq!(p.conflicts().len(), 1);
        p.pick(EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        assert_eq!(p.conflicts().len(), 1);
    }

    #[test]
    fn pick_overwrite_keeps_last() {
        let mut p = progress(vec![conflict("alpha", BlobType::Data, 1, 2)]);
        let id = EntryId::new("alpha".to_string()).unwrap();
        p.pick(id.clone(), Side::Local).unwrap();
        p.pick(id.clone(), Side::Incoming).unwrap();
        let r = p.resolve().unwrap();
        assert_eq!(r.pick_for(&id), Some(Side::Incoming));
    }
}
