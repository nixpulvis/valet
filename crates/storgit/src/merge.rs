//! Shared merge types used by every layout.
//!
//! The merge kernels live next to each layout (see
//! [`crate::layout::subdir`] and [`crate::layout::submodule`]); this
//! module defines the cross-cutting vocabulary they speak.
//!
//! Both kernels drive [`Layout::apply`] and [`Distribute::pull`]
//! and return a [`MergeStatus`]. A `Clean` status means the merge
//! is already finalised. A `Conflicted` status carries a
//! [`MergeProgress`] the caller drives: inspect
//! [`MergeProgress::conflicts`], call [`MergeProgress::pick`] for
//! each, then [`MergeProgress::resolve`] to produce an [`Outcome`]
//! that [`Merge::merge`] accepts.
//!
//! [`Layout::apply`]: crate::Layout::apply
//! [`Distribute::pull`]: crate::Distribute::pull

use std::collections::HashMap;

use crate::error::Error;
use crate::id::CommitId;
use crate::id::EntryId;

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

/// One ref/entry that advanced as a result of a merge. Subdir
/// produces entries with `id = None` (the single shared ref);
/// submodule produces one per per-id submodule HEAD that moved.
#[derive(Debug, Clone)]
pub struct FastForward {
    /// The entry that advanced, or `None` for a layout-wide
    /// (single-ref) advance.
    pub id: Option<EntryId>,
    /// New commit on that ref.
    pub commit: CommitId,
}

/// Outcome of [`Layout::apply`](crate::Layout::apply) or
/// [`Distribute::pull`](crate::Distribute::pull).
#[derive(Debug)]
pub enum MergeStatus {
    /// Merge finalised; the list reports what moved in the local
    /// store. Empty for a no-op apply.
    Clean(Vec<FastForward>),
    /// Merge paused on conflicts. Drive the progress through
    /// `pick` + `resolve`, then call [`Merge::merge`].
    Conflicted(MergeProgress),
}

/// In-progress merge state: the conflicts surfaced by the kernel plus
/// the caller's picks so far. The kernel's non-conflicting planned
/// ops are persisted in each layout's bare repo -- subdir saves the
/// auto-merged tree oid in `MERGE_TREE`; submodule writes a real gix
/// index at `parent.git/index` whose stage-0 entries encode the
/// resolved gitlink set and stages 1/2/3 encode each conflict's
/// base/local/incoming oids -- not on this struct.
#[derive(Debug)]
pub struct MergeProgress {
    conflicts: Vec<Conflict>,
    picks: HashMap<EntryId, Side>,
}

impl MergeProgress {
    pub(crate) fn new(conflicts: Vec<Conflict>) -> Self {
        Self {
            conflicts,
            picks: HashMap::new(),
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
    pub fn pick(&mut self, id: &EntryId, side: Side) -> Result<(), Error> {
        if !self.conflicts.iter().any(|c| &c.id == id) {
            return Err(Error::Other(format!("pick: id {id} is not in conflicts")));
        }
        self.picks.insert(id.clone(), side);
        Ok(())
    }

    /// Validate and consume. Returns `self` back unchanged when any
    /// conflict is still unpicked so the caller can record more picks
    /// and retry; inspect [`remaining`](Self::remaining) to see what's
    /// left.
    pub fn resolve(self) -> Result<Outcome, Self> {
        if self
            .conflicts
            .iter()
            .any(|c| !self.picks.contains_key(&c.id))
        {
            return Err(self);
        }
        Ok(Outcome { picks: self.picks })
    }
}

/// Validated merge resolution. The only constructor is
/// [`MergeProgress::resolve`]; the only consumer is
/// [`Merge::merge`].
#[derive(Debug)]
pub struct Outcome {
    pub(crate) picks: HashMap<EntryId, Side>,
}

impl Outcome {
    /// The pick recorded for `id`, if any.
    pub fn pick_for(&self, id: &EntryId) -> Option<Side> {
        self.picks.get(id).copied()
    }
}

/// Resolve-and-finalise primitives shared by both layouts. The
/// network-driven entry point (`pull`) lives on
/// [`crate::Distribute`].
pub trait Merge: crate::Layout {
    /// True when a merge is in progress and the operator still needs
    /// to call [`Self::merge`] or [`Self::abort`].
    fn merge_in_progress(&self) -> bool;

    /// Abort an in-progress merge, clearing any `MERGE_HEAD` markers.
    /// No-op when no merge is in progress.
    fn abort(&mut self) -> Result<(), Error>;

    /// Finalise an in-progress merge using the picks in `resolution`.
    fn merge(&mut self, resolution: Outcome) -> Result<Vec<FastForward>, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn progress(conflicts: Vec<Conflict>) -> MergeProgress {
        MergeProgress::new(conflicts)
    }

    #[test]
    fn pick_unknown_id_errors() {
        let mut p = progress(vec![conflict("alpha", BlobType::Data, 1, 2)]);
        let other = EntryId::new("beta".to_string()).unwrap();
        assert!(p.pick(&other, Side::Local).is_err());
    }

    #[test]
    fn resolve_partial_errors() {
        let mut p = progress(vec![
            conflict("alpha", BlobType::Data, 1, 2),
            conflict("beta", BlobType::Label, 3, 4),
        ]);
        p.pick(&EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        assert!(p.resolve().is_err());
    }

    #[test]
    fn resolve_full_success() {
        let mut p = progress(vec![
            conflict("alpha", BlobType::Data, 1, 2),
            conflict("beta", BlobType::Label, 3, 4),
        ]);
        p.pick(&EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        p.pick(&EntryId::new("beta".to_string()).unwrap(), Side::Incoming)
            .unwrap();
        let r = p.resolve().ok().unwrap();
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
        p.pick(&EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        assert_eq!(p.remaining().len(), 1);
        assert_eq!(p.remaining()[0].id.as_str(), "beta");
    }

    #[test]
    fn conflicts_is_stable_under_picks() {
        let mut p = progress(vec![conflict("alpha", BlobType::Data, 1, 2)]);
        assert_eq!(p.conflicts().len(), 1);
        p.pick(&EntryId::new("alpha".to_string()).unwrap(), Side::Local)
            .unwrap();
        assert_eq!(p.conflicts().len(), 1);
    }

    #[test]
    fn pick_overwrite_keeps_last() {
        let mut p = progress(vec![conflict("alpha", BlobType::Data, 1, 2)]);
        let id = EntryId::new("alpha".to_string()).unwrap();
        p.pick(&id, Side::Local).unwrap();
        p.pick(&id, Side::Incoming).unwrap();
        let r = p.resolve().ok().unwrap();
        assert_eq!(r.pick_for(&id), Some(Side::Incoming));
    }
}
