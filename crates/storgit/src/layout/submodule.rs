//! The submodule layout: a parent bare repo whose tree carries one
//! gitlink per live entry plus a `.gitmodules` manifest, and one
//! bare submodule repo per entry id with its own object database.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
use crate::layout::Layout;
use crate::module::{read_entry_at, write_entry_commit, write_tombstone_commit};
use crate::parent::{GITMODULES_FILE, INDEX_DIR, load_parent_state, serialize_gitmodules};
use crate::store::Store;
use crate::tarball::{tar_dir, untar_into};

/// Parent bare repo directory inside the scratch dir.
const PARENT_DIR: &str = "parent.git";
/// Directory holding per-entry submodule bare repos inside the scratch
/// dir. Each `<id>.git/` is a full bare repo with its own objects.
const MODULES_DIR: &str = "modules";

/// Map from entry [`Id`] to the bytes of that module's tarball.
pub type Modules = HashMap<Id, Vec<u8>>;

/// Caller-supplied lookup for module bytes. Called by
/// [`SubmoduleLayout`] when an operation first touches an id whose
/// bytes are neither on disk nor in [`Parts::modules`]. A fetcher is
/// assumed to reflect live backing storage at call time.
///
/// Return values:
/// - `Ok(Some(bytes))` - module exists, here are its tarball bytes.
/// - `Ok(None)` - no such module in backing storage. If the id is
///   live in the parent's gitlink set, this surfaces as an error
///   (the caller's backing store has diverged from the parent);
///   otherwise the id is treated as fresh.
/// - `Err(e)` - lookup itself failed; the op fails with
///   [`crate::Error::Fetch`].
pub type ModuleFetcher = Arc<
    dyn Fn(&Id) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync + 'static>>
        + Send
        + Sync,
>;

/// The persisted form of a submodule-layout [`Store`].
///
/// Feed this to [`Store::<SubmoduleLayout>::open`] to reconstruct a
/// store. An empty `parent` means "fresh store"; the first
/// [`Store::<SubmoduleLayout>::snapshot`] will emit the
/// newly-initialised state so the caller can persist it.
///
/// The presence or absence of [`Parts::fetcher`] decides whether the
/// resulting store is lazy:
///
/// - No fetcher: the caller promises [`Parts::modules`] lists every
///   live module. A miss for a live id at `ensure_loaded` time is an
///   error; a miss for an unknown id is treated as fresh.
/// - With a fetcher: [`Parts::modules`] is a prewarm cache consulted
///   first; misses fall through to the fetcher, whose answer is
///   authoritative.
pub struct Parts {
    /// Tarball of the parent bare repo.
    pub parent: Vec<u8>,
    /// Tarball of each submodule bare repo the caller has in hand,
    /// keyed by entry [`Id`]. With no fetcher, this is the complete
    /// set of live modules; with a fetcher, it's an optional prewarm
    /// cache consulted before the fetcher.
    pub modules: Modules,
    /// Optional backing-store lookup. See [`ModuleFetcher`].
    pub fetcher: Option<ModuleFetcher>,
}

impl std::fmt::Debug for Parts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Parts")
            .field("parent_bytes", &self.parent.len())
            .field("modules", &self.modules.keys().collect::<Vec<_>>())
            .field(
                "fetcher",
                &self.fetcher.as_ref().map(|_| "..").unwrap_or("None"),
            )
            .finish()
    }
}

impl Default for Parts {
    /// A fresh, empty [`Parts`] with no fetcher. Equivalent to
    /// `Parts { parent: Vec::new(), modules: Modules::new(), fetcher: None }`.
    fn default() -> Self {
        Parts {
            parent: Vec::new(),
            modules: Modules::new(),
            fetcher: None,
        }
    }
}

impl Parts {
    /// Fold a [`Snapshot`] delta into this [`Parts`]. After applying,
    /// `self` reflects the store's state at the moment the snapshot was
    /// taken, and can be fed straight back into
    /// [`Store::<SubmoduleLayout>::open`].
    ///
    /// - A `Some` parent in the snapshot overwrites [`Parts::parent`].
    /// - [`ModuleChange::Changed`] overwrites or inserts the module.
    /// - [`ModuleChange::Deleted`] removes the module if present.
    pub fn apply(&mut self, snap: Snapshot) {
        if let Some(parent) = snap.parent {
            self.parent = parent;
        }
        for (id, change) in snap.modules {
            match change {
                ModuleChange::Changed(bytes) => {
                    self.modules.insert(id, bytes);
                }
                ModuleChange::Deleted => {
                    self.modules.remove(&id);
                }
            }
        }
    }
}

impl From<Snapshot> for Parts {
    /// Build a fresh [`Parts`] (no fetcher) by calling [`Parts::apply`]
    /// on an empty one with `snap`.
    ///
    /// Use this only when the snapshot is known to describe the entire store,
    /// not a delta against something already persisted. In practice that means
    /// the first snapshot taken from a store opened with [`Parts::default`]:
    /// every module is dirty, the parent is dirty, and there are no deletions
    /// to express because there was nothing there to delete.
    ///
    /// For any subsequent snapshot, apply it to the [`Parts`] you already have.
    /// A plain conversion there would drop every module not mentioned in the
    /// delta, since each snapshot only contains the changes from the last.
    fn from(snap: Snapshot) -> Self {
        let mut parts = Parts::default();
        parts.apply(snap);
        parts
    }
}

/// The delta produced by [`Store::<SubmoduleLayout>::snapshot`]: only
/// the parts touched since the previous snapshot (or, for the first
/// call, since [`Store::<SubmoduleLayout>::open`]).
#[derive(Debug, Default)]
pub struct Snapshot {
    /// `Some` when the parent tarball changed and should be repersisted.
    pub parent: Option<Vec<u8>>,
    /// Touched submodules only. Ids absent from this map are
    /// unchanged in storage.
    pub modules: HashMap<Id, ModuleChange>,
}

/// What happened to a submodule between two snapshots.
#[derive(Debug, Clone)]
pub enum ModuleChange {
    /// The submodule's tarball; write it to storage.
    Changed(Vec<u8>),
    /// The submodule was deleted; drop it from storage.
    Deleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleDirt {
    Changed,
    Deleted,
}

/// The submodule-layout storgit implementation. Owns a scratch
/// [`TempDir`] containing the parent bare repo and any modules that
/// have been loaded. Dropping the layout without calling
/// [`Store::<SubmoduleLayout>::snapshot`] discards uncommitted state.
pub struct SubmoduleLayout {
    scratch: TempDir,
    dirty_parent: bool,
    dirty_modules: HashMap<Id, ModuleDirt>,
    gitlinks: BTreeMap<Id, gix::ObjectId>,
    label_cache: BTreeMap<Id, Vec<u8>>,
    gitlinks_dirty: bool,
    pending_modules: Mutex<Modules>,
    fetcher: Option<ModuleFetcher>,
}

impl SubmoduleLayout {
    fn parent_path(&self) -> PathBuf {
        self.scratch.path().join(PARENT_DIR)
    }

    fn module_path(&self, id: &Id) -> PathBuf {
        self.scratch
            .path()
            .join(MODULES_DIR)
            .join(format!("{}.git", id.as_str()))
    }

    fn current_module_commit(&self, id: &Id) -> Option<gix::ObjectId> {
        self.gitlinks.get(id).copied()
    }

    fn mark_module_changed(&mut self, id: &Id) {
        self.dirty_parent = true;
        self.dirty_modules.insert(id.clone(), ModuleDirt::Changed);
    }

    fn mark_module_deleted(&mut self, id: &Id) {
        self.dirty_parent = true;
        self.dirty_modules.insert(id.clone(), ModuleDirt::Deleted);
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

    /// Materialise the current gitlinks + label_cache into one parent
    /// commit. No-op when nothing has changed since the last
    /// materialisation.
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
        entries.sort_by(|a, b| a.filename.cmp(&b.filename));

        let tree = Tree { entries };
        self.commit_parent_tree(&parent, tree)?;
        self.gitlinks_dirty = false;
        self.dirty_parent = true;
        Ok(())
    }

    /// Write `tree` and a new orphan commit pointing at it into the
    /// parent's own object DB, then overwrite the parent's
    /// `refs/heads/main` to point at that commit. Prunes the loose
    /// objects that the new commit supersedes.
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

impl Layout for SubmoduleLayout {
    fn put(
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
                self.label_cache.remove(id);
            }
            None => {}
        }
        self.gitlinks_dirty = true;
        self.mark_module_changed(id);
        Ok(Some(module_commit.into()))
    }

    fn get(&self, id: &Id) -> Result<Option<Entry>, Error> {
        let Some(commit) = self.current_module_commit(id) else {
            return Ok(None);
        };
        self.ensure_loaded(id)?;
        let repo = gix::open(self.module_path(id))?;
        Ok(Some(read_entry_at(&repo, commit)?))
    }

    fn archive(&mut self, id: &Id) -> Result<(), Error> {
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

    fn delete(&mut self, id: &Id) -> Result<(), Error> {
        let mod_path = self.module_path(id);
        if mod_path.exists() {
            std::fs::remove_dir_all(&mod_path)?;
        }
        self.pending_modules.lock().unwrap().remove(id);
        if self.gitlinks.remove(id).is_some() {
            self.label_cache.remove(id);
            self.gitlinks_dirty = true;
            self.mark_module_deleted(id);
        }
        Ok(())
    }

    fn list(&self) -> Result<Vec<Id>, Error> {
        Ok(self.gitlinks.keys().cloned().collect())
    }

    fn history(&self, id: &Id) -> Result<Vec<Entry>, Error> {
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

    fn label(&self, id: &Id) -> Option<&[u8]> {
        self.label_cache.get(id).map(Vec::as_slice)
    }

    fn list_labels(&self) -> Vec<(Id, Vec<u8>)> {
        self.label_cache
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

// --- Submodule-layout-specific inherent methods on Store<SubmoduleLayout> ---

impl Store<SubmoduleLayout> {
    /// Create a fresh, empty submodule-layout store. Shorthand for
    /// `Store::open(Parts::default())`.
    pub fn new() -> Result<Self, Error> {
        Self::open(Parts::default())
    }

    /// Open a submodule-layout store from its persisted [`Parts`].
    /// Pass [`Parts::default`] to create a fresh, empty store.
    pub fn open(parts: Parts) -> Result<Self, Error> {
        let scratch = tempfile::Builder::new().prefix("storgit-").tempdir()?;
        std::fs::create_dir_all(scratch.path().join(MODULES_DIR))?;

        let dirty_parent = parts.parent.is_empty();
        if dirty_parent {
            init_bare_on_branch(&scratch.path().join(PARENT_DIR))?;
        } else {
            untar_into(&parts.parent, &scratch.path().join(PARENT_DIR))?;
        }

        let pending_modules = Mutex::new(parts.modules);

        let (gitlinks, label_cache) = if dirty_parent {
            (BTreeMap::new(), BTreeMap::new())
        } else {
            load_parent_state(&scratch.path().join(PARENT_DIR))?
        };

        Ok(Store {
            layout: SubmoduleLayout {
                scratch,
                dirty_parent,
                dirty_modules: HashMap::new(),
                gitlinks,
                label_cache,
                gitlinks_dirty: false,
                pending_modules,
                fetcher: parts.fetcher,
            },
        })
    }

    /// Re-tar everything touched since the previous snapshot (or since
    /// [`Store::<SubmoduleLayout>::open`] for the first call) and hand
    /// the caller exactly the parts that need repersisting. Clears
    /// dirty tracking on success, so back-to-back snapshots with no
    /// intervening writes return an empty [`Snapshot`].
    pub fn snapshot(&mut self) -> Result<Snapshot, Error> {
        self.layout.flush_parent()?;
        let mut snap = Snapshot::default();
        if self.layout.dirty_parent {
            snap.parent = Some(tar_dir(&self.layout.parent_path())?);
        }
        let dirty = std::mem::take(&mut self.layout.dirty_modules);
        for (name, state) in dirty {
            let change = match state {
                ModuleDirt::Changed => {
                    let bytes = tar_dir(&self.layout.module_path(&name))?;
                    ModuleChange::Changed(bytes)
                }
                ModuleDirt::Deleted => ModuleChange::Deleted,
            };
            snap.modules.insert(name, change);
        }
        self.layout.dirty_parent = false;
        Ok(snap)
    }

    /// Inverse of [`Store::<SubmoduleLayout>::save`]: reconstruct a
    /// store from a tarball previously produced by `save`. Passing an
    /// empty slice is *not* supported here; use
    /// [`Store::<SubmoduleLayout>::open`] with an empty [`Parts`] for a
    /// fresh store.
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
            layout: SubmoduleLayout {
                scratch,
                dirty_parent: false,
                dirty_modules: HashMap::new(),
                gitlinks,
                label_cache,
                gitlinks_dirty: false,
                pending_modules: Mutex::new(Modules::new()),
                fetcher: None,
            },
        })
    }

    /// Bundle the entire store into a single self-contained tarball.
    /// Force-loads every live module so the returned tarball is
    /// self-consistent.
    pub fn save(&mut self) -> Result<Vec<u8>, Error> {
        self.layout.flush_parent()?;
        let live: Vec<Id> = self.layout.gitlinks.keys().cloned().collect();
        for id in &live {
            self.layout.ensure_loaded(id)?;
        }
        tar_dir(self.layout.scratch.path())
    }

    /// Make `bytes` (a previously-persisted module tarball) available
    /// to the store under `id`. The next operation that touches `id`
    /// will untar these bytes to the scratch dir on first access.
    pub fn load_module(&mut self, id: Id, bytes: Vec<u8>) {
        self.layout
            .pending_modules
            .get_mut()
            .unwrap()
            .insert(id, bytes);
    }
}
