//! The submodule layout, i.e. `<path>/parent.git/` +
//! `<path>/modules/<id>/{data,label}`

mod merge;
mod module;
pub(crate) mod parent;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use gix::bstr::BString;
use gix::objs::{
    Tree,
    tree::{Entry as TreeEntry, EntryKind},
};

use self::module::ModuleRepo;
use self::parent::{GITMODULES_FILE, INDEX_DIR, ParentTree, serialize_gitmodules};
use crate::entry::Entry;
use crate::error::Error;
use crate::git::{BareRepo, init_bare_on_branch, write_commit};
use crate::id::CommitId;
use crate::id::EntryId;
use crate::layout::Layout;
use crate::merge::{ApplyMode, MergeStatus};
use crate::tarball::{tar_dir, untar_into};

/// Parent bare repo directory inside the scratch dir.
pub(crate) const PARENT_DIR: &str = "parent.git";
/// Directory holding per-entry submodule bare repos inside the scratch
/// dir. Each `<id>.git/` is a full bare repo with its own objects.
pub(crate) const MODULES_DIR: &str = "modules";

/// On-disk path to the per-id module bare repo under
/// `<root>/modules/<id>.git`. The single source of truth for the
/// modules-directory layout; every other call site routes through
/// here (or [`SubmoduleLayout::module_dir`] for the same path
/// resolved against `self.path`).
pub(crate) fn module_dir_at(root: &std::path::Path, id: &EntryId) -> PathBuf {
    root.join(MODULES_DIR).join(format!("{}.git", id.as_str()))
}

/// Initialise the per-id module bare repo under `<root>/modules/<id>.git`
/// if it doesn't already exist. No-op when present.
pub(crate) fn ensure_module_repo(root: &std::path::Path, id: &EntryId) -> Result<(), Error> {
    let mod_path = module_dir_at(root, id);
    if !mod_path.exists() {
        init_bare_on_branch(&mod_path)?;
    }
    Ok(())
}

/// Derive the per-module fetch URL from the parent's remote URL.
/// Convention: parent URLs end in `/parent.git`; replace that segment
/// with `/modules/<id>.git`. Works for any scheme (file, ssh, https).
/// Errors if `parent_url` doesn't end in `/parent.git`.
pub(crate) fn derive_module_url(parent_url: &str, id: &EntryId) -> Result<String, Error> {
    let trimmed = parent_url.trim_end_matches('/');
    let base = trimmed.strip_suffix("/parent.git").ok_or_else(|| {
        Error::Other(format!(
            "remote URL {parent_url:?} does not end in /parent.git; \
             cannot derive module URLs"
        ))
    })?;
    Ok(format!("{base}/modules/{}.git", id.as_str()))
}

/// Map from entry [`EntryId`] to the bytes of that module's tarball.
pub type Modules = HashMap<EntryId, Vec<u8>>;

/// Caller-supplied lookup for module bytes. Called by
/// [`SubmoduleLayout`] when an operation first touches an id whose
/// bytes are neither on disk nor previously seeded via
/// [`SubmoduleLayout::with_bundle`]. A fetcher is assumed to reflect
/// live backing storage at call time. Install one with
/// [`SubmoduleLayout::with_fetcher`].
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
    dyn Fn(&EntryId) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync + 'static>>
        + Send
        + Sync,
>;

/// The persisted, transferable form of a submodule-layout
/// [`Store`][crate::Store]. Pairs with [`Layout::apply`] (consumer)
/// and [`Layout::bundle`] (producer).
///
/// Feed this to [`SubmoduleLayout::with_bundle`] to seed a store
/// with previously-persisted bytes. An empty `parent` means "fresh
/// store"; the first [`Layout::bundle`] call will emit the
/// newly-initialised state so the caller can persist it.
///
/// Re-bundling after writes returns a *delta* against the previous
/// bundle: only `parent` (when changed) and only the touched
/// `modules`. Persistence layers fold each bundle into their own
/// state by overwriting `parent` (when non-empty), upserting each
/// `modules` entry, and dropping every id in `deleted`.
///
/// Whether the store is lazy is an orthogonal concern, controlled by
/// [`SubmoduleLayout::with_fetcher`]:
///
/// - No fetcher: the caller promises [`Bundle::modules`] lists every
///   live module. A miss for a live id at `ensure_loaded` time is an
///   error; a miss for an unknown id is treated as fresh.
/// - With a fetcher: [`Bundle::modules`] is a prewarm cache consulted
///   first; misses fall through to the fetcher, whose answer is
///   authoritative.
#[derive(Debug, Default, Clone)]
pub struct Bundle {
    /// Tarball of the parent bare repo. Empty means "no change":
    /// either a fresh store with nothing to publish yet, or a
    /// re-bundle after no parent-touching writes.
    pub parent: Vec<u8>,
    /// Tarball of each submodule bare repo the caller has in hand,
    /// keyed by entry [`EntryId`]. From [`Layout::bundle`] this is
    /// only the modules touched since the previous bundle. From a
    /// caller seeding via [`SubmoduleLayout::with_bundle`] without a
    /// fetcher, it must list every live module; with a fetcher it's
    /// an optional prewarm cache consulted before the fetcher.
    pub modules: Modules,
    /// Ids hard-deleted since the previous bundle. The persistence
    /// pipeline drops these from backing storage. The merge pipeline
    /// ignores `deleted` (see `TODO-sync-deletes.md`).
    pub deleted: Vec<EntryId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModuleDirt {
    Changed,
    Deleted,
}

/// Backed by a directory at `path`, containing `parent.git/` (a bare parent
/// repo) and `modules/<id>.git/` (one bare submodule per entry).
///
/// # Disk layout
///
/// ```text
/// <path>/
///   parent.git/           bare repo: the store's index
///     HEAD -> refs/heads/main
///     refs/heads/main     -> the single parent commit
///   modules/
///     <id>.git/           bare repo: one per live entry, its own object DB
/// ```
///
/// # Parent tree
///
/// The parent repo's `refs/heads/main` points at a single commit whose
/// tree holds:
///
/// - one gitlink per live entry, filename is the entry's [`EntryId`], oid is the
///   submodule's head commit
/// - `.gitmodules`, a serialised manifest of the gitlink set, so the parent
///   is a valid git submodule parent that `git submodule` can drive
/// - `index/`, a subtree of blobs keyed by [`EntryId`] carrying each entry's
///   label bytes. This mirrors the label cache without having to clone the
///   entry's submodule to read it
///
/// A _gitlink_ is a tree entry whose blob is actually a commit ID in another
/// repository. Git treats it as a pointer to that commit rather than as file
/// content, which is how submodules pin a specific revision of a nested repo
/// from their parent's tree.
///
/// The parent keeps a real commit chain: each flush writes a new commit
/// whose parent is the prior HEAD. This lets `gix::merge_base` find the
/// common ancestor between two parent histories during sync, which the
/// merge kernel uses to disambiguate adds from archives on each side.
///
/// # Per-entry repos
///
/// Each `modules/<id>.git/` has its own commit chain for that entry alone.
/// [`Layout::put`] appends a commit carrying a `label` blob and/or a
/// `data` blob, [`Layout::archive`] appends a tombstone commit and
/// drops the gitlink from the parent, and [`Layout::history`] walks
/// that chain. The
/// entry-level history is independent of the parent, so pruning gitlinks
/// never rewrites per-entry history.
///
/// # Lazy loading
///
/// A module directory only needs to exist on disk when an operation
/// touches it. `ensure_loaded` materialises an entry's bare repo from
/// either a preloaded tarball in `pending_modules` or the [`ModuleFetcher`]
/// installed via [`SubmoduleLayout::with_fetcher`], letting a
/// store back a large entry set without keeping every repo on disk.
pub struct SubmoduleLayout {
    path: PathBuf,
    dirty_parent: bool,
    dirty_modules: HashMap<EntryId, ModuleDirt>,
    gitlinks: BTreeMap<EntryId, gix::ObjectId>,
    label_cache: BTreeMap<EntryId, Vec<u8>>,
    gitlinks_dirty: bool,
    pending_modules: Mutex<Modules>,
    fetcher: Option<ModuleFetcher>,
}

impl SubmoduleLayout {
    /// Bare git directory for the parent repo (the one carrying the
    /// gitlinks tree, label cache, and the configured remotes).
    pub fn parent_dir(&self) -> PathBuf {
        self.path.join(PARENT_DIR)
    }

    /// Bare git directory for `id`'s module repo. Single source of
    /// truth for the modules-directory layout (rooted at this
    /// layout's `path`).
    pub fn module_dir(&self, id: &EntryId) -> PathBuf {
        module_dir_at(&self.path, id)
    }

    pub(crate) fn root_path(&self) -> &std::path::Path {
        &self.path
    }

    pub(crate) fn gitlinks(&self) -> &BTreeMap<EntryId, gix::ObjectId> {
        &self.gitlinks
    }

    pub(crate) fn set_gitlink(&mut self, id: EntryId, oid: gix::ObjectId) {
        self.gitlinks.insert(id.clone(), oid);
        self.label_cache.remove(&id);
        self.gitlinks_dirty = true;
        self.mark_module_changed(&id);
    }

    pub(crate) fn refresh_label_for(&mut self, id: &EntryId, label: Option<Vec<u8>>) {
        match label {
            Some(b) if !b.is_empty() => {
                self.label_cache.insert(id.clone(), b);
            }
            _ => {
                self.label_cache.remove(id);
            }
        }
    }

    fn current_module_commit(&self, id: &EntryId) -> Option<gix::ObjectId> {
        self.gitlinks.get(id).copied()
    }

    pub(crate) fn mark_module_changed(&mut self, id: &EntryId) {
        self.dirty_parent = true;
        self.dirty_modules.insert(id.clone(), ModuleDirt::Changed);
    }

    pub(crate) fn mark_module_deleted(&mut self, id: &EntryId) {
        self.dirty_parent = true;
        self.dirty_modules.insert(id.clone(), ModuleDirt::Deleted);
    }

    /// Ensure `id`'s module bytes are extracted into its bare repo
    /// dir under this layout. The single entry point for materialising
    /// modules from any source.
    ///
    /// - Already on disk: no-op.
    /// - `bytes` is `Some`: untar those bytes. Cheaper than going
    ///   through the prewarm cache when the caller has bytes in
    ///   hand (e.g. just-decrypted from a backing store).
    /// - `bytes` is `None`: storgit looks for the bytes in the
    ///   prewarm cache populated by [`Bundle::modules`], then falls
    ///   back to the installed [`ModuleFetcher`] (if any). Errors
    ///   when neither produces bytes and `id` is live in the parent
    ///   (the on-disk state would have diverged from the parent's
    ///   gitlink set).
    pub fn ensure_loaded(&self, id: &EntryId, bytes: Option<Vec<u8>>) -> Result<(), Error> {
        let mod_path = self.module_dir(id);
        if mod_path.exists() {
            return Ok(());
        }
        if let Some(bytes) = bytes {
            untar_into(&bytes, &mod_path)?;
            return Ok(());
        }
        let pending = self.pending_modules.lock().unwrap().remove(id);
        if let Some(bytes) = pending {
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
                "module {} is live but its bytes are not loaded; pass them \
                 to SubmoduleLayout::ensure_loaded or install a ModuleFetcher",
                id.as_str()
            )));
        }
        Ok(())
    }

    /// Materialise the current gitlinks + label_cache into one parent
    /// commit. No-op when nothing has changed since the last
    /// materialisation.
    pub(crate) fn flush_parent(&mut self) -> Result<(), Error> {
        if !self.gitlinks_dirty {
            return Ok(());
        }
        let parent = gix::open(self.parent_dir())?;

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

    /// Write `tree` and a new commit pointing at it into the parent's
    /// own object DB, then overwrite the parent's `refs/heads/main` to
    /// point at that commit. The new commit chains to whatever the
    /// parent's prior HEAD was (if any), so the parent has a real
    /// commit history that `gix::merge_base` can walk during sync.
    fn commit_parent_tree(&self, parent: &gix::Repository, tree: Tree) -> Result<(), Error> {
        let parent_path = self.parent_dir();
        let br = BareRepo::new(&parent_path);
        let prior = br.read_head()?;
        let parents: Vec<gix::ObjectId> = prior.into_iter().collect();
        let tree_id = parent.write_object(&tree)?.detach();
        let commit_id = write_commit(parent, tree_id, parents, "parent")?;
        br.write_head(commit_id)?;
        Ok(())
    }
}

/// Transient accumulator the submodule merge kernel uses while it
/// classifies the gitlink union. Holds the non-conflict decisions
/// (advances + archives) until `run_merge_kernel` decides whether
/// to apply them inline (clean path) or serialise them into the
/// parent.git index alongside the conflict stages (conflict path).
/// Never crosses a function boundary.
#[derive(Debug, Default)]
pub(crate) struct PlannedOps {
    /// Set the gitlink for `id` to this oid.
    pub(crate) advances: HashMap<EntryId, gix::ObjectId>,
    /// Archive `id` locally (write tombstone, drop gitlink). Used
    /// when the remote archived an entry the local side hadn't
    /// modified since the merge base.
    pub(crate) archives: Vec<EntryId>,
}

impl Layout for SubmoduleLayout {
    type Bundle = Bundle;

    fn git_dir(&self) -> PathBuf {
        self.parent_dir()
    }

    fn new(path: PathBuf) -> Result<Self, Error> {
        if path.exists() {
            return Err(Error::Other(format!(
                "submodule new: path {path:?} already exists"
            )));
        }
        std::fs::create_dir(&path)?;
        std::fs::create_dir(path.join(MODULES_DIR))?;
        init_bare_on_branch(&path.join(PARENT_DIR))?;
        Ok(SubmoduleLayout {
            path,
            dirty_parent: true,
            dirty_modules: HashMap::new(),
            gitlinks: BTreeMap::new(),
            label_cache: BTreeMap::new(),
            gitlinks_dirty: false,
            pending_modules: Mutex::new(Modules::new()),
            fetcher: None,
        })
    }

    fn open(path: PathBuf) -> Result<Self, Error> {
        validate_submodule_repo(&path)?;
        let parent_path = path.join(PARENT_DIR);
        let modules_path = path.join(MODULES_DIR);
        if !modules_path.exists() {
            std::fs::create_dir(&modules_path)?;
        }
        let ParentTree { gitlinks, labels } = ParentTree::load(&parent_path)?;
        Ok(SubmoduleLayout {
            path,
            dirty_parent: false,
            dirty_modules: HashMap::new(),
            gitlinks,
            label_cache: labels,
            gitlinks_dirty: false,
            pending_modules: Mutex::new(Modules::new()),
            fetcher: None,
        })
    }

    fn save_tar(&mut self) -> Result<Vec<u8>, Error> {
        self.flush_parent()?;
        let live: Vec<EntryId> = self.gitlinks.keys().cloned().collect();
        for id in &live {
            self.ensure_loaded(id, None)?;
        }
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
        if BareRepo::new(&self.parent_dir()).merge_in_progress() {
            return Err(Error::Other(
                "Layout::put: merge in progress; resolve or abort first".into(),
            ));
        }
        self.ensure_loaded(id, None)?;
        let mod_path = self.module_dir(id);
        if !mod_path.exists() {
            init_bare_on_branch(&mod_path)?;
        }
        let module = gix::open(&mod_path)?;
        let Some(module_commit) = ModuleRepo::new(&module).write_entry(label, data)? else {
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

    fn get(&self, id: &EntryId) -> Result<Option<Entry>, Error> {
        let Some(commit) = self.current_module_commit(id) else {
            return Ok(None);
        };
        self.ensure_loaded(id, None)?;
        let repo = gix::open(self.module_dir(id))?;
        Ok(Some(ModuleRepo::new(&repo).read_entry(commit)?))
    }

    fn archive(&mut self, id: &EntryId) -> Result<bool, Error> {
        if !self.gitlinks.contains_key(id) {
            return Ok(false);
        }
        self.ensure_loaded(id, None)?;
        let module = gix::open(self.module_dir(id))?;
        ModuleRepo::new(&module).write_tombstone()?;
        self.gitlinks.remove(id);
        self.label_cache.remove(id);
        self.gitlinks_dirty = true;
        self.mark_module_changed(id);
        Ok(true)
    }

    fn delete(&mut self, id: &EntryId) -> Result<(), Error> {
        let mod_path = self.module_dir(id);
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

    fn list(&self) -> Result<Vec<EntryId>, Error> {
        Ok(self.gitlinks.keys().cloned().collect())
    }

    fn history(&self, id: &EntryId) -> Result<Vec<Entry>, Error> {
        self.ensure_loaded(id, None)?;
        let mod_path = self.module_dir(id);
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
            out.push(ModuleRepo::new(&repo).read_entry(info.id)?);
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

    /// Re-tar everything touched since the previous bundle (or since
    /// [`Layout::open`] for the first call). Empty `parent` means
    /// no parent advance since the previous bundle; only modules
    /// touched since then appear in `modules` / `deleted`.
    fn bundle(&mut self) -> Result<Bundle, Error> {
        self.flush_parent()?;
        let mut bundle = Bundle::default();
        if self.dirty_parent {
            bundle.parent = tar_dir(&self.parent_dir())?;
        }
        let dirty = std::mem::take(&mut self.dirty_modules);
        for (name, state) in dirty {
            match state {
                ModuleDirt::Changed => {
                    let bytes = tar_dir(&self.module_dir(&name))?;
                    bundle.modules.insert(name, bytes);
                }
                ModuleDirt::Deleted => {
                    bundle.deleted.push(name);
                }
            }
        }
        self.dirty_parent = false;
        Ok(bundle)
    }

    /// Fold `bundle` into this layout. With [`ApplyMode::Merge`] the
    /// kernel runs and may surface conflicts; with
    /// [`ApplyMode::FastForwardOnly`] divergent gitlinks return
    /// [`Error::NotFastForward`]. An empty `bundle.parent` is a
    /// clean no-op (no incoming state to merge).
    fn apply(&mut self, bundle: Bundle, mode: ApplyMode) -> Result<MergeStatus, Error> {
        if bundle.parent.is_empty() {
            return Ok(MergeStatus::Clean(Vec::new()));
        }
        let Some(incoming_parent) = self.import_bundle(&bundle)? else {
            return Ok(MergeStatus::Clean(Vec::new()));
        };
        let parent_path = self.parent_dir();
        let incoming_gitlinks = ParentTree::gitlinks_at(&parent_path, incoming_parent)?;

        if mode == ApplyMode::FastForwardOnly {
            let diverging = crate::layout::submodule::merge::preflight_ff_check(
                self.gitlinks(),
                &incoming_gitlinks,
                |id| self.module_dir(id),
            )?;
            if !diverging.is_empty() {
                return Err(Error::NotFastForward { ids: diverging });
            }
        }

        self.run_merge_kernel(incoming_parent, incoming_gitlinks)
    }
}

impl Drop for SubmoduleLayout {
    /// Materialise any pending parent commit to disk. Per-module
    /// commits are already persistent (every `put` writes the commit
    /// and updates the module's ref before returning), so only the
    /// gitlink-set / label-cache batched in memory needs flushing.
    ///
    /// Errors are swallowed -- Drop can't surface them. Callers who
    /// need error-handling should call `bundle` or `save`
    /// explicitly.
    fn drop(&mut self) {
        let _ = self.flush_parent();
    }
}

/// Sanity-check that `path` holds a valid submodule-layout storgit
/// repo: a directory containing `parent.git/` as a bare repo whose
/// HEAD points to storgit's branch. `modules/` may or may not exist
/// yet (it's created on demand), so its presence is not required.
fn validate_submodule_repo(path: &std::path::Path) -> Result<(), Error> {
    if !path.is_dir() {
        return Err(Error::Other(format!(
            "submodule open: {path:?} is not a directory"
        )));
    }
    let parent_path = path.join(PARENT_DIR);
    if !parent_path.is_dir() {
        return Err(Error::Other(format!(
            "submodule open: {parent_path:?} does not exist"
        )));
    }
    gix::open(&parent_path).map_err(|e| {
        Error::Other(format!(
            "submodule open: {parent_path:?} is not a git repo: {e}"
        ))
    })?;
    BareRepo::new(&parent_path).validate_head_branch("submodule open")
}

// --- Submodule-layout-specific inherent methods ---

impl SubmoduleLayout {
    /// Seed an opened submodule layout with previously-persisted
    /// [`Bundle`] bytes.
    ///
    /// - `bundle.parent`: if non-empty, untarred into `parent.git`,
    ///   replacing whatever `open` produced. Requires the existing
    ///   parent to be empty (freshly initialised). TODO: allow
    ///   applying parent bytes onto an existing non-empty parent
    ///   by routing through [`Layout::apply`].
    /// - `bundle.modules`: inserted into the pending-modules cache;
    ///   each id's tarball is untarred on first touch.
    /// - `bundle.deleted`: ignored. `with_bundle` is for seeding
    ///   from already-pruned storage; nothing is "live" yet for
    ///   `deleted` to remove.
    pub fn with_bundle(mut self, bundle: Bundle) -> Result<Self, Error> {
        if !bundle.parent.is_empty() {
            // TODO: merge bundle.parent with the on-disk parent when
            // the latter already has history. For now, require the
            // on-disk parent to be freshly init'd (dirty_parent is
            // the in-memory signal for that state).
            if !self.dirty_parent {
                return Err(Error::Other(
                    "with_bundle: bundle.parent is non-empty but the layout's parent.git already \
                     has state; merging is not yet implemented"
                        .into(),
                ));
            }
            let parent_path = self.parent_dir();
            if parent_path.exists() {
                std::fs::remove_dir_all(&parent_path)?;
            }
            untar_into(&bundle.parent, &parent_path)?;
            let ParentTree { gitlinks, labels } = ParentTree::load(&parent_path)?;
            self.gitlinks = gitlinks;
            self.label_cache = labels;
            self.dirty_parent = false;
        }
        let pending = self.pending_modules.get_mut().unwrap();
        for (id, bytes) in bundle.modules {
            pending.insert(id, bytes);
        }
        Ok(self)
    }

    /// Install a [`ModuleFetcher`] as a lazy fallback for module
    /// bytes, replacing any prior fetcher. See [`ModuleFetcher`] for
    /// the contract.
    pub fn with_fetcher(mut self, fetcher: ModuleFetcher) -> Self {
        self.fetcher = Some(fetcher);
        self
    }
}
