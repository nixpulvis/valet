use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Mutex;

use gix::bstr::BString;
use gix::objs::{
    Tree,
    tree::{Entry as TreeEntry, EntryKind},
};
use tempfile::TempDir;

use crate::entry::{CommitId, Entry};
use crate::error::Error;
use crate::git::{
    drop_loose_object, init_bare_on_branch, module_ref_path, read_ref_file, write_commit,
    write_ref_file,
};
use crate::id::Id;
use crate::module::{read_entry_at, write_entry_commit, write_tombstone_commit};
use crate::parent::{GITMODULES_FILE, INDEX_DIR, load_parent_state, serialize_gitmodules};
use crate::persist::{ModuleChange, ModuleFetcher, Modules, Parts, Snapshot};
use crate::tarball::{tar_dir, untar_into};

/// Parent bare repo directory inside the scratch dir. Holds the
/// gitlink tree, `.gitmodules`, the `index/` label cache, and its
/// own objects.
const PARENT_DIR: &str = "parent.git";
/// Directory holding per-entry submodule bare repos inside the scratch
/// dir. Each `<id>.git/` is a full bare repo with its own objects.
const MODULES_DIR: &str = "modules";

/// Handle to a storgit store. Owns a scratch [`TempDir`] containing
/// the parent bare repo and any modules that have been loaded.
/// Dropping the handle without calling [`Store::snapshot`] discards
/// uncommitted state.
pub struct Store {
    scratch: TempDir,
    /// Set to `true` when the parent tarball needs to be re-emitted on
    /// the next [`Store::snapshot`]. Covers both "freshly opened" and
    /// "at least one mutation has touched the gitlink set".
    dirty_parent: bool,
    /// Per-module dirtiness since the last [`Store::snapshot`]. Ids
    /// missing from this map were not touched.
    dirty_modules: HashMap<Id, ModuleDirt>,
    /// Live id -> module-commit map. Authoritative in-memory view of
    /// the parent's gitlink set. Mutations update this map directly;
    /// the parent's tree + orphan commit are materialised from it
    /// lazily (on [`Store::snapshot`] or [`Store::save`]) so a batch
    /// of N puts produces one parent commit instead of N.
    gitlinks: BTreeMap<Id, gix::ObjectId>,
    /// Live id -> encoded label blob. Mirrors the `label` file inside
    /// each module's HEAD commit, kept in the parent so callers can
    /// search / list without opening every submodule. Ids with an
    /// empty label are absent from this map (and from the on-disk
    /// `index/` subtree); their modules still exist in [`Store::gitlinks`].
    label_cache: BTreeMap<Id, Vec<u8>>,
    /// Set when [`Store::gitlinks`] or [`Store::label_cache`] has
    /// diverged from the parent ref on disk. Cleared by the
    /// materialisation pass that writes a single parent tree + commit
    /// reflecting the current state.
    gitlinks_dirty: bool,
    /// Module tarballs the caller has handed us but that haven't been
    /// extracted to the scratch dir yet. Populated from
    /// [`Parts::modules`] on [`Store::open`] and from
    /// [`Store::load_module`] afterwards; drained by `ensure_loaded`
    /// when an operation first touches a given module (and
    /// unconditionally on [`Store::save`], which needs every live
    /// module on disk to bundle).
    /// `Mutex` (rather than `RefCell`) so `Store` is `Sync`. The map is
    /// only briefly touched inside `ensure_loaded` / `load_module` /
    /// `delete`, so contention is a non-issue in practice, and `Sync`
    /// lets async callers hold a `&Store` across `.await` boundaries.
    pending_modules: Mutex<Modules>,
    /// Optional lookup consulted by `ensure_loaded` when
    /// `pending_modules` and the scratch dir both miss. Carries the
    /// [`Parts::fetcher`] supplied at open time; `None` means the
    /// caller promised [`Parts::modules`] was exhaustive.
    fetcher: Option<ModuleFetcher>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleDirt {
    /// The module exists on disk and should be re-tarred on the next
    /// snapshot. Covers both brand-new modules and ones that gained a
    /// new commit.
    Changed,
    /// The module's directory has been removed. The next snapshot will
    /// report [`ModuleChange::Deleted`] so the caller drops it from
    /// persistence.
    Deleted,
}

impl Store {
    /// Create a fresh, empty store. Shorthand for
    /// `Store::open(Parts::default())`.
    pub fn new() -> Result<Self, Error> {
        Store::open(Parts::default())
    }

    /// Open a store from its persisted [`Parts`]. Pass [`Parts::default`]
    /// to create a fresh, empty store; the first [`Store::snapshot`]
    /// will then emit the newly-initialised parts so the caller can
    /// persist them.
    pub fn open(parts: Parts) -> Result<Self, Error> {
        let scratch = tempfile::Builder::new().prefix("storgit-").tempdir()?;
        std::fs::create_dir_all(scratch.path().join(MODULES_DIR))?;

        let dirty_parent = parts.parent.is_empty();
        if dirty_parent {
            init_bare_on_branch(&scratch.path().join(PARENT_DIR))?;
        } else {
            untar_into(&parts.parent, &scratch.path().join(PARENT_DIR))?;
        }

        // Modules are stashed for lazy untar; see `ensure_loaded`.
        let pending_modules = Mutex::new(parts.modules);

        let (gitlinks, label_cache) = if dirty_parent {
            (BTreeMap::new(), BTreeMap::new())
        } else {
            load_parent_state(&scratch.path().join(PARENT_DIR))?
        };

        Ok(Store {
            scratch,
            dirty_parent,
            dirty_modules: HashMap::new(),
            gitlinks,
            label_cache,
            gitlinks_dirty: false,
            pending_modules,
            fetcher: parts.fetcher,
        })
    }

    /// Re-tar everything touched since the previous snapshot (or since
    /// [`Store::open`] for the first call) and hand the caller exactly
    /// the parts that need repersisting. Clears dirty tracking on
    /// success, so back-to-back snapshots with no intervening writes
    /// return an empty [`Snapshot`].
    pub fn snapshot(&mut self) -> Result<Snapshot, Error> {
        self.flush_parent()?;
        let mut snap = Snapshot::default();
        if self.dirty_parent {
            snap.parent = Some(tar_dir(&self.scratch.path().join(PARENT_DIR))?);
        }
        let dirty = std::mem::take(&mut self.dirty_modules);
        for (name, state) in dirty {
            let change = match state {
                ModuleDirt::Changed => {
                    let bytes = tar_dir(&self.module_path(&name))?;
                    ModuleChange::Changed(bytes)
                }
                ModuleDirt::Deleted => ModuleChange::Deleted,
            };
            snap.modules.insert(name, change);
        }
        self.dirty_parent = false;
        Ok(snap)
    }

    /// Inverse of [`Store::save`]: reconstruct a store from a tarball
    /// previously produced by `save`. Passing an empty slice is *not*
    /// supported here; use [`Store::open`] with an empty [`Parts`] for
    /// a fresh store.
    pub fn load(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Err(Error::Other(
                "Store::load requires a non-empty tarball; use Store::open for a fresh store"
                    .into(),
            ));
        }
        let scratch = tempfile::Builder::new().prefix("storgit-").tempdir()?;
        untar_into(bytes, scratch.path())?;
        if !scratch.path().join(PARENT_DIR).exists() {
            return Err(Error::Other("tarball missing parent.git directory".into()));
        }
        std::fs::create_dir_all(scratch.path().join(MODULES_DIR))?;
        let (gitlinks, label_cache) = load_parent_state(&scratch.path().join(PARENT_DIR))?;
        Ok(Store {
            scratch,
            dirty_parent: false,
            dirty_modules: HashMap::new(),
            gitlinks,
            label_cache,
            gitlinks_dirty: false,
            pending_modules: Mutex::new(Modules::new()),
            fetcher: None,
        })
    }

    /// Bundle the entire store into a single self-contained tarball.
    ///
    /// Prefer [`Store::open`] + [`Store::snapshot`] when the caller has
    /// somewhere to persist parts individually: incremental writes
    /// touch only the parent tarball and the changed module. `save` is
    /// for the simpler case where the caller just wants one opaque
    /// BLOB back.
    ///
    /// `save` materialises any pending parent commit and force-loads
    /// every live module (so the returned tarball is self-consistent).
    /// It does not clear the per-module dirty tracking that
    /// [`Store::snapshot`] consumes. Callers can continue to mutate
    /// the store afterwards, or mix `save` and [`Store::snapshot`]
    /// freely.
    pub fn save(&mut self) -> Result<Vec<u8>, Error> {
        self.flush_parent()?;
        // Force-load every live module so the bundle is complete.
        let live: Vec<Id> = self.gitlinks.keys().cloned().collect();
        for id in &live {
            self.ensure_loaded(id)?;
        }
        tar_dir(self.scratch.path())
    }

    /// Write a new version of entry `id` whose commit tree carries the
    /// given `label` and/or `data` files. Each side is independently
    /// optional: `None` means "leave that slot unchanged, reusing
    /// whatever blob the module's current HEAD tree has there";
    /// `Some(bytes)` writes a new blob (possibly zero-length) under
    /// that name. For a brand-new module with no prior commit, `None`
    /// means "no blob in that slot" since there is nothing to reuse.
    /// Creates the submodule on first call; subsequent calls append a
    /// commit to the submodule's history and advance the parent's
    /// gitlink for `id` to the new commit.
    ///
    /// Returns `Ok(None)` when the new commit's tree would be
    /// byte-identical to the module's current HEAD tree: storgit
    /// skips the commit rather than record a no-op. With the reuse
    /// semantics above, `put(id, Some(label), None)` is always a
    /// no-op when `label` matches what's already there, and likewise
    /// for data-only updates.
    ///
    /// Rejects `put(id, None, None)` as an error: use [`Store::archive`]
    /// to write a tombstone. Passing both sides `None` here would
    /// otherwise overlap with the tombstone representation.
    pub fn put(
        &mut self,
        id: &Id,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error> {
        if label.is_none() && data.is_none() {
            return Err(Error::Other(
                "Store::put requires at least one of label or data; use Store::archive for a tombstone"
                    .into(),
            ));
        }
        // Materialise the existing module's history before extending
        // it (no-op if dir already on disk or this id is brand-new).
        self.ensure_loaded(id)?;
        let mod_path = self.module_path(id);
        if !mod_path.exists() {
            init_bare_on_branch(&mod_path)?;
        }
        let module = gix::open(&mod_path)?;
        let Some(module_commit) = write_entry_commit(&module, label, data)? else {
            return Ok(None);
        };
        self.gitlinks.insert(id.clone(), module_commit);
        match label {
            Some(bytes) if !bytes.is_empty() => {
                self.label_cache.insert(id.clone(), bytes.to_vec());
            }
            Some(_) => {
                // Explicit empty label: clear the cache entry.
                self.label_cache.remove(id);
            }
            None => {
                // Reuse prior label blob: leave the cache entry alone.
            }
        }
        self.gitlinks_dirty = true;
        self.mark_module_changed(id);
        Ok(Some(module_commit.into()))
    }

    fn mark_module_changed(&mut self, id: &Id) {
        self.dirty_parent = true;
        self.dirty_modules.insert(id.clone(), ModuleDirt::Changed);
    }

    fn mark_module_deleted(&mut self, id: &Id) {
        self.dirty_parent = true;
        self.dirty_modules.insert(id.clone(), ModuleDirt::Deleted);
    }

    /// Return the latest [`Entry`] for `id`, or `None` if `id` is not
    /// a live entry (archived, deleted, or never written). The
    /// returned entry's `label` and `data` are each `Some` iff that
    /// file was present in the HEAD commit's tree.
    pub fn get(&self, id: &Id) -> Result<Option<Entry>, Error> {
        let Some(commit) = self.current_module_commit(id)? else {
            return Ok(None);
        };
        self.ensure_loaded(id)?;
        let repo = gix::open(self.module_path(id))?;
        Ok(Some(read_entry_at(&repo, commit)?))
    }

    /// Return the current label blob for `id`, or `None` if `id` is
    /// not a live entry or its label is absent/empty (both cases are
    /// omitted from the parent's index cache). Cheap: reads the
    /// in-memory cache, no module open required.
    pub fn label(&self, id: &Id) -> Option<&[u8]> {
        self.label_cache.get(id).map(Vec::as_slice)
    }

    /// Return every live entry whose label is non-empty, paired with
    /// that label blob. Entries whose label is absent or empty are
    /// omitted. Cheap: reads the in-memory cache, no module opens
    /// required.
    pub fn list_labels(&self) -> Vec<(Id, Vec<u8>)> {
        self.label_cache
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Soft-delete `id`: append a tombstone commit (empty tree) to the
    /// submodule and remove `id` from the parent's live entry set.
    /// The submodule's bare repo stays in the archive, so [`Store::history`]
    /// still walks both the old payloads and the tombstone. A later
    /// [`Store::put`] for the same `id` resumes the existing history.
    pub fn archive(&mut self, id: &Id) -> Result<(), Error> {
        if !self.gitlinks.contains_key(id) {
            return Ok(());
        }
        self.ensure_loaded(id)?;
        let module = gix::open(self.module_path(id))?;
        write_tombstone_commit(&module)?;
        self.gitlinks.remove(id);
        self.label_cache.remove(id);
        self.gitlinks_dirty = true;
        self.mark_module_changed(id);
        Ok(())
    }

    /// Hard-delete `id`: drop the submodule's bare repo from the
    /// archive entirely and remove `id` from the parent. The history
    /// is gone; a later [`Store::put`] with the same `id` starts a
    /// fresh, unrelated history. Use [`Store::archive`] instead if you
    /// want to preserve the trail of past payloads.
    pub fn delete(&mut self, id: &Id) -> Result<(), Error> {
        let mod_path = self.module_path(id);
        if mod_path.exists() {
            std::fs::remove_dir_all(&mod_path)?;
        }
        // Drop any pending bytes too: don't accidentally resurrect
        // a deleted entry on the next ensure_loaded.
        self.pending_modules.lock().unwrap().remove(id);
        if self.gitlinks.remove(id).is_some() {
            self.label_cache.remove(id);
            self.gitlinks_dirty = true;
            self.mark_module_deleted(id);
        }
        Ok(())
    }

    /// List the ids of all live entries in arbitrary order.
    pub fn list(&self) -> Result<Vec<Id>, Error> {
        Ok(self.gitlinks.keys().cloned().collect())
    }

    /// Walk every historical version of `id`, newest first. Returns an
    /// empty vec if `id` was never written. Deleted entries whose
    /// submodule repo still exists in the archive will still return
    /// their past commits here; filter by [`Store::list`] first if you
    /// want live-only history.
    pub fn history(&self, id: &Id) -> Result<Vec<Entry>, Error> {
        self.ensure_loaded(id)?;
        let mod_path = self.module_path(id);
        if !mod_path.exists() {
            return Ok(Vec::new());
        }
        let repo = gix::open(&mod_path)?;
        let Ok(head) = repo.head_commit() else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for info in head.ancestors().all()? {
            let info = info?;
            out.push(read_entry_at(&repo, info.id)?);
        }
        Ok(out)
    }

    /// Make `bytes` (a previously-persisted module tarball) available
    /// to the store under `id`. The next operation that touches `id`
    /// (`get`, `history`, `put`, `archive`) will untar these bytes to
    /// the scratch dir on first access.
    ///
    /// Use this as an explicit push when neither [`Parts::modules`]
    /// nor a [`Parts::fetcher`] fits: e.g. the caller just decoded a
    /// module out of band and wants it resident without re-entering
    /// the fetcher. For persistence layouts that store modules
    /// separately (one row per module), prefer giving [`Parts`] a
    /// [`ModuleFetcher`]; `ensure_loaded` will call it on first
    /// touch.
    pub fn load_module(&mut self, id: Id, bytes: Vec<u8>) {
        self.pending_modules.get_mut().unwrap().insert(id, bytes);
    }

    /// Ensure `id`'s module is extracted to the scratch dir. If the
    /// dir already exists, no-op. Otherwise consume any pending
    /// tarball for `id` and untar it. If nothing is pending, fall back
    /// to the [`ModuleFetcher`] when one was supplied via
    /// [`Parts::fetcher`]. If neither path produces bytes and the id
    /// is live in the parent, return an error so the caller knows the
    /// backing store has diverged from the parent's gitlink set.
    fn ensure_loaded(&self, id: &Id) -> Result<(), Error> {
        let mod_path = self.module_path(id);
        if mod_path.exists() {
            return Ok(());
        }
        let bytes = self.pending_modules.lock().unwrap().remove(id);
        if let Some(bytes) = bytes {
            untar_into(&bytes, &mod_path)?;
            return Ok(());
        }
        if let Some(fetcher) = &self.fetcher {
            match fetcher(id).map_err(Error::Fetch)? {
                Some(bytes) => {
                    untar_into(&bytes, &mod_path)?;
                    return Ok(());
                }
                None => {
                    if self.gitlinks.contains_key(id) {
                        return Err(Error::Other(format!(
                            "module {} is live in the parent but fetcher returned None",
                            id.as_str()
                        )));
                    }
                    // Not live and fetcher has nothing: treat as fresh.
                    return Ok(());
                }
            }
        }
        if self.gitlinks.contains_key(id) {
            return Err(Error::Other(format!(
                "module {} is live but its bytes are not loaded; call Store::load_module first",
                id.as_str()
            )));
        }
        Ok(())
    }

    fn parent_path(&self) -> PathBuf {
        self.scratch.path().join(PARENT_DIR)
    }

    fn module_path(&self, id: &Id) -> PathBuf {
        self.scratch
            .path()
            .join(MODULES_DIR)
            .join(format!("{}.git", id.as_str()))
    }

    /// Commit oid currently referenced by the parent's gitlink for `id`,
    /// or `None` if the parent has no live entry with that id.
    fn current_module_commit(&self, id: &Id) -> Result<Option<gix::ObjectId>, Error> {
        Ok(self.gitlinks.get(id).copied())
    }

    /// Materialise the current [`Store::gitlinks`] + [`Store::label_cache`]
    /// into one parent commit. The root tree carries:
    ///
    /// - every gitlink (mode `160000`) for the live entries,
    /// - a `.gitmodules` blob describing those gitlinks so plain
    ///   `git` tooling on an extracted tarball recognises them as
    ///   submodules (skipped when there are no gitlinks),
    /// - and an [`INDEX_DIR`] subtree of label-cache blobs (skipped
    ///   when no label is set).
    ///
    /// Updates the parent's `refs/heads/main`. No-op when nothing has
    /// changed since the last materialisation.
    fn flush_parent(&mut self) -> Result<(), Error> {
        if !self.gitlinks_dirty {
            return Ok(());
        }
        let parent = gix::open(self.parent_path())?;

        let mut entries: Vec<TreeEntry> = self
            .gitlinks
            .iter()
            .map(|(id, oid)| TreeEntry {
                mode: EntryKind::Commit.into(),
                filename: BString::from(id.as_str()),
                oid: *oid,
            })
            .collect();

        if !self.gitlinks.is_empty() {
            let manifest = serialize_gitmodules(&self.gitlinks);
            let blob_id = parent.write_blob(&manifest)?.detach();
            entries.push(TreeEntry {
                mode: EntryKind::Blob.into(),
                filename: BString::from(GITMODULES_FILE),
                oid: blob_id,
            });
        }

        if !self.label_cache.is_empty() {
            let mut index_entries: Vec<TreeEntry> = Vec::with_capacity(self.label_cache.len());
            for (id, bytes) in &self.label_cache {
                let blob_id = parent.write_blob(bytes)?.detach();
                index_entries.push(TreeEntry {
                    mode: EntryKind::Blob.into(),
                    filename: BString::from(id.as_str()),
                    oid: blob_id,
                });
            }
            // BTreeMap iteration is already sorted by filename, which is
            // what git requires for tree entries.
            let index_tree = Tree {
                entries: index_entries,
            };
            let index_tree_id = parent.write_object(&index_tree)?.detach();
            entries.push(TreeEntry {
                mode: EntryKind::Tree.into(),
                filename: BString::from(INDEX_DIR),
                oid: index_tree_id,
            });
        }
        // Gitlinks sort together with the `index` subtree entry; git
        // requires strict byte-wise sort by filename.
        entries.sort_by(|a, b| a.filename.cmp(&b.filename));

        let tree = Tree { entries };
        self.commit_parent_tree(&parent, tree)?;
        self.gitlinks_dirty = false;
        self.dirty_parent = true;
        Ok(())
    }

    /// Write `tree` and a new orphan commit pointing at it into the
    /// parent's own object DB, then overwrite the parent's
    /// `refs/heads/main` to point at that commit.
    ///
    /// Storgit doesn't keep parent history; the per-entry module
    /// histories are the product. The previous parent commit and its
    /// tree become unreachable garbage as soon as we update the ref.
    /// To keep `parent.git/objects/` from growing linearly with total
    /// puts, we delete those two loose objects directly after
    /// publishing the new ref. No reachability walker needed: we know
    /// exactly which two objects were just superseded.
    fn commit_parent_tree(&self, parent: &gix::Repository, tree: Tree) -> Result<(), Error> {
        let superseded = self.superseded_parent_objects(parent)?;
        let tree_id = parent.write_object(&tree)?.detach();
        let commit_id = write_commit(parent, tree_id, Vec::new(), "parent")?;
        write_ref_file(&module_ref_path(&self.parent_path()), commit_id)?;

        if let Some((old_commit, old_tree)) = superseded {
            if old_commit != commit_id {
                drop_loose_object(&self.parent_path(), old_commit)?;
            }
            if let Some(old_tree) = old_tree
                && old_tree != tree_id
            {
                drop_loose_object(&self.parent_path(), old_tree)?;
            }
        }
        Ok(())
    }

    /// Return the parent commit currently at `refs/heads/main` and the
    /// tree it points to, if any. Used to identify the objects about
    /// to be superseded by a new parent commit.
    fn superseded_parent_objects(
        &self,
        parent: &gix::Repository,
    ) -> Result<Option<(gix::ObjectId, Option<gix::ObjectId>)>, Error> {
        let Some(commit_oid) = read_ref_file(&module_ref_path(&self.parent_path()))? else {
            return Ok(None);
        };
        let tree_oid = parent
            .find_object(commit_oid)
            .ok()
            .and_then(|o| o.try_into_commit().ok())
            .and_then(|c| c.decode().ok().map(|d| d.tree()));
        Ok(Some((commit_oid, tree_oid)))
    }
}
