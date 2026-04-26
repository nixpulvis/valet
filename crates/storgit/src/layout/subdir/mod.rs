//! The subdir layout, i.e. `path/records/<id>/{data,label}`.

mod merge;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::{
    Tree,
    tree::{Entry as TreeEntry, EntryKind},
};

use crate::entry::Entry;
use crate::error::Error;
use crate::git::{
    BareRepo, LABEL_FILE, build_slot_entries, decode_tree, init_bare_on_branch, write_commit,
};
use crate::id::CommitId;
use crate::id::EntryId;
use crate::layout::Layout;
use crate::merge::{ApplyMode, MergeStatus};
use crate::tarball::tar_dir;

/// Subtree name inside the repo's root tree that holds every entry.
const RECORDS_DIR: &str = "records";

/// The persisted, transferable form of a subdir-layout
/// [`Store`][crate::Store]. Pairs with [`Layout::apply`] (consumer)
/// and [`Layout::bundle`] (producer).
///
/// Subdir keeps its whole state in one bare repo, so the bundle
/// carries one tarball of that repo plus the ids hard-deleted since
/// the previous bundle.
#[derive(Debug, Default, Clone)]
pub struct Bundle {
    /// Tarball of the bare repo. Empty means "no change since the
    /// previous bundle": either a fresh store with no commits, or a
    /// re-bundle after no writes.
    pub repo: Vec<u8>,
    /// Ids hard-deleted since the previous bundle. The persistence
    /// pipeline drops these from backing storage.
    ///
    /// Subdir's `delete` today archives (single shared ref can't
    /// cheaply rewrite history; see `TODO-sync-deletes.md`); the
    /// receiver should log a warning that history-rewrite isn't
    /// implemented yet and proceed.
    pub deleted: Vec<EntryId>,
}

/// Backed by a single bare repo at `path`. Every entry lives as a
/// subtree of that repo's root tree, and a single shared ref
/// advances with every write.
///
/// # Disk layout
///
/// ```text
/// <path>/                 bare repo
///   HEAD -> refs/heads/main
///   refs/heads/main       -> the latest commit
///   objects/              the shared object DB for every entry
/// ```
///
/// # Tree shape
///
/// The tip commit's root tree has a single `records/` subtree. Inside
/// `records/`, each live entry is a subtree named by its [`EntryId`]
/// carrying up to two blobs:
///
/// - `data`, the entry's payload bytes
/// - `label`, the entry's label bytes (omitted when the label is empty)
///
/// ```text
/// <root>
///   records/
///     <id-a>/
///       data
///       label
///     <id-b>/
///       data
/// ```
///
/// `DATA_FILE` sorts before `LABEL_FILE`, so entry subtrees are
/// assembled in that order to satisfy git's strict filename sort
/// without an explicit sort call.
///
/// # Write path
///
/// Each `put` or `archive` builds a new `records/<id>/` subtree,
/// splices it into a new `records/` tree (copying the other ids
/// unchanged), wraps that in a new root tree, and commits with the
/// prior tip as parent. The new commit is published by rewriting
/// `refs/heads/main` to point at it. `put` is a no-op when the
/// rebuilt `records/<id>/` subtree is byte-identical to the prior
/// one; in that case no commit is written and the ref is unchanged.
/// `delete` runs the same in-repo write as `archive` (a single
/// shared ref can't cheaply excise one entry's history without
/// rewriting every commit that touched it) and additionally
/// records the id in `pending_deletes`, so the next [`Layout::bundle`]
/// surfaces it in `Bundle.deleted` for the persistence layer to
/// drop from backing storage.
///
/// # Per-entry history
///
/// There are no per-entry refs. [`Layout::history`]
/// walks the shared commit graph and emits an [`Entry`] for each
/// commit where the `records/<id>/` subtree differs from its parent's,
/// which mirrors what `git log -- records/<id>/` would surface. The
/// walk reads the subtree oid at each commit and compares it to the
/// first parent's, so it's O(history length) but each step is a
/// constant-time oid compare.
///
/// # Label cache
///
/// [`Layout::label`] and [`Layout::list_labels`] are served from an
/// in-memory `label_cache` keyed by [`EntryId`]. The cache is populated on
/// [`Layout::open`] by walking `HEAD:records/` once, and kept current
/// by `put` and `archive`.
pub struct SubdirLayout {
    path: PathBuf,
    label_cache: BTreeMap<EntryId, Vec<u8>>,
    /// True when a `put`, `archive`, `delete`, `apply`, or `merge`
    /// has advanced HEAD since the last [`Layout::bundle`]. Drives
    /// the bundle's `repo` slot.
    dirty: bool,
    /// Hard-delete intent accumulated since the last [`Layout::bundle`].
    /// `delete()` archives in storgit (subdir can't cheaply rewrite
    /// history) and pushes the id here; the next bundle drains it
    /// into [`Bundle::deleted`].
    ///
    /// NOTE: this is in-memory only. A process killed between
    /// `delete()` and `bundle()` loses the deletion intent: the
    /// archive commit is durable in the repo, but the persistence
    /// layer never gets told to drop the id from backing storage.
    /// Durable tracking would need a real ref or sidecar file in
    /// `<git_dir>/storgit/`; revisit alongside the broader
    /// process-kill durability story.
    pending_deletes: Vec<EntryId>,
}

impl SubdirLayout {
    fn open_repo(&self) -> Result<gix::Repository, Error> {
        Ok(gix::open(&self.path)?)
    }

    fn bare(&self) -> BareRepo<'_> {
        BareRepo::new(&self.path)
    }

    fn head_commit(&self, _repo: &gix::Repository) -> Result<Option<gix::ObjectId>, Error> {
        self.bare().read_head()
    }

    /// Refresh derived state after HEAD advanced via something
    /// other than `put`/`archive` (e.g. a merge commit).
    pub(crate) fn rebuild_after_advance(&mut self) -> Result<(), Error> {
        self.dirty = true;
        self.rebuild_label_cache()
    }

    /// Build and persist the label_cache by walking `HEAD:records/`.
    fn rebuild_label_cache(&mut self) -> Result<(), Error> {
        self.label_cache.clear();
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(());
        };
        let rt = RecordsTree::at_commit(&repo, Some(head))?;
        let Some(records_oid) = rt.records_oid()? else {
            return Ok(());
        };
        let records_tree = decode_tree(&repo, records_oid)?;
        for entry in records_tree.entries {
            if !matches!(entry.mode.kind(), EntryKind::Tree) {
                continue;
            }
            let id = EntryId::new(entry.filename.to_string()).map_err(|e| {
                Error::Other(format!("corrupt records/ entry {:?}: {e}", entry.filename))
            })?;
            let id_tree = decode_tree(&repo, entry.oid)?;
            if let Some(label_entry) = id_tree
                .entries
                .iter()
                .find(|e| e.filename.as_bstr() == BStr::new(LABEL_FILE))
            {
                let blob = repo.find_object(label_entry.oid)?;
                if !blob.data.is_empty() {
                    self.label_cache.insert(id, blob.data.clone());
                }
            }
        }
        Ok(())
    }
}

/// A view on the subdir layout's fixed tree shape at some commit:
/// `root -> records/ -> <id>/`. Groups the navigation and mutation
/// helpers that would otherwise each thread `prior_root_tree_id` /
/// `prior_records_oid` / `prior_id_subtree_oid` as separate args.
///
/// `root_tree` is `None` when the commit doesn't exist yet (fresh
/// repo); every lookup through a `None` root just returns `None`.
pub(super) struct RecordsTree<'r> {
    repo: &'r gix::Repository,
    root_tree: Option<gix::ObjectId>,
}

impl<'r> RecordsTree<'r> {
    /// View at `commit`. `None` means no commit has landed yet.
    pub(super) fn at_commit(
        repo: &'r gix::Repository,
        commit: Option<gix::ObjectId>,
    ) -> Result<Self, Error> {
        let root_tree = match commit {
            Some(c) => Some(repo.find_object(c)?.into_commit().decode()?.tree()),
            None => None,
        };
        Ok(Self { repo, root_tree })
    }

    /// View at `root_tree` directly. Used by the merge kernel when it
    /// has a tree oid in hand without needing to redecode the commit.
    pub(super) fn at_root(repo: &'r gix::Repository, root_tree: Option<gix::ObjectId>) -> Self {
        Self { repo, root_tree }
    }

    /// `records/` subtree oid at this root, if any.
    pub(super) fn records_oid(&self) -> Result<Option<gix::ObjectId>, Error> {
        match self.root_tree {
            Some(t) => subtree_entry(self.repo, t, RECORDS_DIR),
            None => Ok(None),
        }
    }

    /// `records/<id>/` subtree oid, if any.
    pub(super) fn id_subtree(&self, id: &EntryId) -> Result<Option<gix::ObjectId>, Error> {
        let Some(records) = self.records_oid()? else {
            return Ok(None);
        };
        subtree_entry(self.repo, records, id.as_str())
    }

    /// Write a new root tree whose `records/<id>/` is set to
    /// `new_id_subtree` (or removed when `None`), preserving every
    /// other root-level entry and every other record. Returns the
    /// new root tree oid.
    pub(super) fn with_id(
        &self,
        id: &EntryId,
        new_id_subtree: Option<gix::ObjectId>,
    ) -> Result<gix::ObjectId, Error> {
        let prior_records = self.records_oid()?;
        let records_tree_id = write_records_tree(self.repo, prior_records, id, new_id_subtree)?;
        write_root_tree(self.repo, self.root_tree, records_tree_id)
    }
}

/// Look up a named child of `tree_id` and return its oid, or `None`
/// if that child doesn't exist. Matches by byte-wise filename.
fn subtree_entry(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    name: &str,
) -> Result<Option<gix::ObjectId>, Error> {
    let tree = decode_tree(repo, tree_id)?;
    Ok(tree
        .entries
        .into_iter()
        .find(|e| e.filename.as_bstr() == BStr::new(name))
        .map(|e| e.oid))
}

impl Layout for SubdirLayout {
    type Bundle = Bundle;

    fn git_dir(&self) -> PathBuf {
        self.path.clone()
    }

    fn new(path: PathBuf) -> Result<Self, Error> {
        if path.exists() {
            return Err(Error::Other(format!(
                "subdir new: path {path:?} already exists"
            )));
        }
        std::fs::create_dir(&path)?;
        init_bare_on_branch(&path)?;
        Ok(SubdirLayout {
            path,
            label_cache: BTreeMap::new(),
            dirty: false,
            pending_deletes: Vec::new(),
        })
    }

    fn open(path: PathBuf) -> Result<Self, Error> {
        validate_subdir_repo(&path)?;
        let mut layout = SubdirLayout {
            path,
            label_cache: BTreeMap::new(),
            dirty: false,
            pending_deletes: Vec::new(),
        };
        layout.rebuild_label_cache()?;
        Ok(layout)
    }

    fn save_tar(&mut self) -> Result<Vec<u8>, Error> {
        tar_dir(&self.path)
    }

    fn put(
        &mut self,
        id: &EntryId,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error> {
        if label.is_none() && data.is_none() {
            return Err(Error::Other(
                "Layout::put requires at least one of label or data; use Layout::archive for a tombstone"
                    .into(),
            ));
        }
        if self.bare().merge_in_progress() {
            return Err(Error::Other(
                "Layout::put: merge in progress; resolve or abort first".into(),
            ));
        }
        let repo = self.open_repo()?;
        let prior_commit = self.head_commit(&repo)?;
        let rt = RecordsTree::at_commit(&repo, prior_commit)?;
        let prior_id_subtree_oid = rt.id_subtree(id)?;
        let prior_id_entries = match prior_id_subtree_oid {
            Some(sid) => Some(decode_tree(&repo, sid)?.entries),
            None => None,
        };
        let id_entries = build_slot_entries(&repo, prior_id_entries.as_deref(), label, data)?;
        let id_tree = Tree {
            entries: id_entries,
        };
        let id_tree_id = repo.write_object(&id_tree)?.detach();

        // No-op detection: identical id-subtree means identical commit
        // for this record.
        if prior_id_subtree_oid == Some(id_tree_id) {
            return Ok(None);
        }

        let root_tree_id = rt.with_id(id, Some(id_tree_id))?;
        let commit_id = write_commit(
            &repo,
            root_tree_id,
            prior_commit.into_iter().collect(),
            "put",
        )?;
        self.bare().write_head(commit_id)?;

        match label {
            Some(bytes) if !bytes.is_empty() => {
                self.label_cache.insert(id.clone(), bytes.to_vec());
            }
            Some(_) => {
                self.label_cache.remove(id);
            }
            None => {}
        }
        self.dirty = true;
        Ok(Some(commit_id.into()))
    }

    fn get(&self, id: &EntryId) -> Result<Option<Entry>, Error> {
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(None);
        };
        let rt = RecordsTree::at_commit(&repo, Some(head))?;
        if rt.id_subtree(id)?.is_none() {
            return Ok(None);
        }
        Ok(Some(read_subdir_entry_at(&repo, head, id)?))
    }

    fn archive(&mut self, id: &EntryId) -> Result<bool, Error> {
        let repo = self.open_repo()?;
        let Some(prior_commit) = self.head_commit(&repo)? else {
            return Ok(false);
        };
        let rt = RecordsTree::at_commit(&repo, Some(prior_commit))?;
        if rt.id_subtree(id)?.is_none() {
            return Ok(false);
        }
        let root_tree_id = rt.with_id(id, None)?;
        let commit_id = write_commit(&repo, root_tree_id, vec![prior_commit], "archive")?;
        self.bare().write_head(commit_id)?;
        self.label_cache.remove(id);
        self.dirty = true;
        Ok(true)
    }

    fn delete(&mut self, id: &EntryId) -> Result<(), Error> {
        // A single-ref layout can't cheaply erase history for one
        // record without rewriting the ref. For now `delete` behaves
        // like `archive`: the entry is removed from the tree, and the
        // past commits that touched it remain reachable from HEAD.
        // The hard-delete intent is recorded in `pending_deletes`
        // (gated on archive actually doing work) so the next
        // `bundle()` carries it through to the persistence layer.
        if self.archive(id)? {
            self.pending_deletes.push(id.clone());
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<EntryId>, Error> {
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(Vec::new());
        };
        let rt = RecordsTree::at_commit(&repo, Some(head))?;
        let Some(records_oid) = rt.records_oid()? else {
            return Ok(Vec::new());
        };
        let records_tree = decode_tree(&repo, records_oid)?;
        let mut out = Vec::with_capacity(records_tree.entries.len());
        for entry in records_tree.entries {
            if !matches!(entry.mode.kind(), EntryKind::Tree) {
                continue;
            }
            let id = EntryId::new(entry.filename.to_string()).map_err(|e| {
                Error::Other(format!("corrupt records/ entry {:?}: {e}", entry.filename))
            })?;
            out.push(id);
        }
        Ok(out)
    }

    fn history(&self, id: &EntryId) -> Result<Vec<Entry>, Error> {
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(Vec::new());
        };
        let head_obj = repo.find_object(head)?.into_commit();
        let mut out = Vec::new();
        for info in head_obj.ancestors().all()? {
            let info = info?;
            let commit = repo.find_object(info.id)?.into_commit();
            let decoded = commit.decode()?;
            let tree_id = decoded.tree();
            let this_subtree = RecordsTree::at_root(&repo, Some(tree_id)).id_subtree(id)?;
            let parent_ids: Vec<gix::ObjectId> = decoded.parents().collect();
            let parent_subtree = if let Some(&pid) = parent_ids.first() {
                let pc = repo.find_object(pid)?.into_commit();
                let ptid = pc.decode()?.tree();
                RecordsTree::at_root(&repo, Some(ptid)).id_subtree(id)?
            } else {
                None
            };
            if this_subtree != parent_subtree {
                out.push(read_subdir_entry_at(&repo, info.id, id)?);
            }
        }
        Ok(out)
    }

    fn label(&self, id: &EntryId) -> Option<&[u8]> {
        self.label_cache.get(id).map(Vec::as_slice)
    }

    fn list_labels(&self) -> Vec<(EntryId, Vec<u8>)> {
        self.label_cache
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Re-tar the bare repo when [`SubdirLayout`] has advanced HEAD
    /// since the previous bundle, otherwise leave `repo` empty. Drains
    /// `pending_deletes` into `bundle.deleted`.
    fn bundle(&mut self) -> Result<Bundle, Error> {
        let repo = if self.dirty {
            tar_dir(&self.path)?
        } else {
            Vec::new()
        };
        let deleted = std::mem::take(&mut self.pending_deletes);
        self.dirty = false;
        Ok(Bundle { repo, deleted })
    }

    /// Fold `bundle` into this layout. With [`ApplyMode::Merge`] the
    /// kernel runs and may surface conflicts; with
    /// [`ApplyMode::FastForwardOnly`] a divergent incoming HEAD
    /// returns [`Error::NotFastForward`] with an empty `ids` (the
    /// single shared ref is rejected layout-wide). An empty
    /// `bundle.repo` is a clean no-op.
    ///
    /// `bundle.deleted` is ignored on the merge side (deletes are a
    /// persistence concern; subdir can't cheaply rewrite history yet
    /// -- see `TODO-sync-deletes.md`).
    fn apply(&mut self, bundle: Bundle, mode: ApplyMode) -> Result<MergeStatus, Error> {
        if bundle.repo.is_empty() {
            return Ok(MergeStatus::Clean(Vec::new()));
        }
        let Some(incoming_head) = crate::tarball::import_tarball_objects(&bundle.repo, &self.path)?
        else {
            return Ok(MergeStatus::Clean(Vec::new()));
        };

        if mode == ApplyMode::FastForwardOnly {
            let local_head = self.bare().read_head()?;
            if let Some(local) = local_head
                && local != incoming_head
            {
                let repo = self.open_repo()?;
                let merge_base = repo
                    .merge_base(local, incoming_head)
                    .ok()
                    .map(|i| i.detach());
                if merge_base != Some(local) {
                    return Err(Error::NotFastForward { ids: Vec::new() });
                }
            }
        }

        self.run_merge_kernel(incoming_head)
    }
}

/// Build a new `records/` subtree by copying the entries of the prior
/// records subtree (if any), removing or replacing the entry for
/// `id`, and writing the result. `new_id_subtree` of `None` removes
/// the id; `Some(oid)` replaces or inserts it.
fn write_records_tree(
    repo: &gix::Repository,
    prior_records_oid: Option<gix::ObjectId>,
    id: &EntryId,
    new_id_subtree: Option<gix::ObjectId>,
) -> Result<Option<gix::ObjectId>, Error> {
    let mut entries: Vec<TreeEntry> = match prior_records_oid {
        Some(roid) => decode_tree(repo, roid)?
            .entries
            .into_iter()
            .filter(|e| e.filename.as_bstr() != BStr::new(id.as_str()))
            .collect(),
        None => Vec::new(),
    };
    if let Some(oid) = new_id_subtree {
        entries.push(TreeEntry {
            mode: EntryKind::Tree.into(),
            filename: BString::from(id.as_str()),
            oid,
        });
    }
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    let tree = Tree { entries };
    Ok(Some(repo.write_object(&tree)?.detach()))
}

/// Build the new root tree by copying prior root entries (if any),
/// removing or replacing the `records/` entry, and writing the
/// result. `new_records` of `None` drops the `records/` subtree.
fn write_root_tree(
    repo: &gix::Repository,
    prior_root_tree_id: Option<gix::ObjectId>,
    new_records: Option<gix::ObjectId>,
) -> Result<gix::ObjectId, Error> {
    let mut entries: Vec<TreeEntry> = match prior_root_tree_id {
        Some(tid) => decode_tree(repo, tid)?
            .entries
            .into_iter()
            .filter(|e| e.filename.as_bstr() != BStr::new(RECORDS_DIR))
            .collect(),
        None => Vec::new(),
    };
    if let Some(oid) = new_records {
        entries.push(TreeEntry {
            mode: EntryKind::Tree.into(),
            filename: BString::from(RECORDS_DIR),
            oid,
        });
    }
    entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    let tree = Tree { entries };
    Ok(repo.write_object(&tree)?.detach())
}

/// Read a single [`Entry`] for `id` at commit `commit_id`. If the
/// `records/<id>/` subtree is absent at that commit (the archive
/// point), returns an entry with both slots `None`.
fn read_subdir_entry_at(
    repo: &gix::Repository,
    commit_id: gix::ObjectId,
    id: &EntryId,
) -> Result<Entry, Error> {
    let rt = RecordsTree::at_commit(repo, Some(commit_id))?;
    let id_subtree_oid = rt.id_subtree(id)?;
    Entry::at_commit_tree(repo, commit_id, id_subtree_oid)
}

/// Sanity-check that `path` holds a valid subdir-layout storgit
/// repo: a bare repo whose HEAD points to storgit's branch and
/// whose root tree (if any commit exists) contains no gitlinks
/// (which would signal a submodule-layout repo).
fn validate_subdir_repo(path: &Path) -> Result<(), Error> {
    let repo = gix::open(path).map_err(|e| {
        Error::Other(format!(
            "storgit subdir open: path {path:?} is not a git repo: {e}"
        ))
    })?;
    let br = BareRepo::new(path);
    br.validate_head_branch("storgit subdir open")?;
    if let Some(commit_oid) = br.read_head()? {
        let commit = repo.find_object(commit_oid)?.into_commit();
        let tree_id = commit.decode()?.tree();
        let tree = decode_tree(&repo, tree_id)?;
        for entry in tree.entries {
            if matches!(entry.mode.kind(), EntryKind::Commit) {
                return Err(Error::Other(format!(
                    "storgit subdir open: {path:?} looks like a submodule-layout storgit repo (gitlinks in root tree)"
                )));
            }
        }
    }
    Ok(())
}

// Subdir layout's new/open/save/load all live on the Layout trait;
// no Store<SubdirLayout> inherent methods are needed.
