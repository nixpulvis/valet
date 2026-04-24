//! Identified, versioned entry storage backed by git.
//!
//! A [`Store`] is two things living under a scratch
//! [`tempfile::TempDir`]:
//!
//! - A full parent bare repo (`parent.git/`) whose tree carries one
//!   gitlink per live entry, a `.gitmodules` manifest, and an
//!   `index/` subtree of label blobs. Holds its own objects.
//! - One full bare submodule per entry id (`modules/<id>.git/`), each
//!   with its own object database; `refs/heads/main` records that
//!   entry's latest commit.
//!
//! There is no shared object database: each module owns its own
//! objects so that a fresh [`Store::open`] can ignore them entirely
//! and the `index/` cache in the parent gives a working label index
//! without touching any submodule. Modules reach the store through
//! one of three paths, all converging on the same on-disk scratch:
//! [`Parts::modules`] (handed over at open time), a
//! [`Parts::fetcher`] consulted on demand for misses, or explicit
//! [`Store::load_module`] pushes after open.
//!
//! Each entry is keyed by an opaque `id` and carries two optional
//! payloads inside every commit: a `label` (searchable metadata the
//! caller wants to scan without opening modules) and a `data` blob
//! (the actual record). `put` writes a commit to that module's
//! objects DB and updates the entry's ref; `get` returns the latest
//! [`Entry`]; `history` walks the entry's commits.
//!
//! Persistence is split per-row. Callers load through [`Parts`]
//! ([`Parts::parent`], [`Parts::modules`]) and persist only what
//! [`Snapshot`] flags as changed, so writing one entry rewrites that
//! one entry's tarball plus the parent's, not every other entry's.
//!
//! storgit is payload-agnostic: it stores raw bytes. Encryption,
//! id policy, label format, and any higher-level semantics belong to
//! the caller.

use std::borrow::Borrow;
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::{
    Commit, Tree,
    tree::{Entry as TreeEntry, EntryKind},
};
use tempfile::TempDir;

/// A validated entry identifier. Constructed via [`Id::new`] (or
/// `s.parse::<Id>()`); the validating constructor enforces every
/// constraint storgit needs to safely use the id as a filename
/// (`modules/<id>.git`), as a git tree entry name, and as a key in
/// the parent tree's gitlink set.
///
/// Rules enforced:
/// - non-empty
/// - at most [`Id::MAX_LEN`] bytes
/// - no `/` (would create a subdirectory in `modules/`)
/// - no `"` or `\` (would need escaping inside the `.gitmodules`
///   section name storgit writes for plain-`git` interop)
/// - no ASCII control characters (`< 0x20` or DEL, including `\0`,
///   `\n`, `\t`); they would corrupt git tree filenames or the
///   `.gitmodules` config file
/// - no leading `.` (rejects `.`, `..`, hidden-file ids)
/// - does not end in `.git` (collides with the `<id>.git` module dir)
/// - is not a [reserved name](Id::is_reserved) used by storgit
///   internally (currently just the string `"index"`, which collides
///   with the parent tree's index subtree)
///
/// Holding an [`Id`] value is therefore proof that the bytes are safe
/// to plug into all of storgit's internal paths and tree writes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(String);

/// Reasons [`Id::new`] can reject a candidate id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    Empty,
    TooLong { len: usize, max: usize },
    BadChar(char),
    LeadingDot,
    GitSuffix,
    Reserved,
}

impl Id {
    /// Maximum byte length of a valid id. Picked to leave headroom
    /// under typical filesystem name limits (255 bytes on most
    /// filesystems) once the `<id>.git` suffix is appended.
    pub const MAX_LEN: usize = 240;

    /// True when `s` is a reserved name storgit uses for its own
    /// bookkeeping inside the parent tree, and therefore must not be
    /// used as an entry id.
    pub fn is_reserved(s: &str) -> bool {
        s == INDEX_DIR
    }

    /// Construct a validated id, returning [`IdError`] on rejection.
    pub fn new(s: impl Into<String>) -> Result<Self, IdError> {
        let s = s.into();
        Self::validate(&s)?;
        Ok(Id(s))
    }

    fn validate(s: &str) -> Result<(), IdError> {
        if s.is_empty() {
            return Err(IdError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(IdError::TooLong {
                len: s.len(),
                max: Self::MAX_LEN,
            });
        }
        if s.starts_with('.') {
            return Err(IdError::LeadingDot);
        }
        if s.ends_with(".git") {
            return Err(IdError::GitSuffix);
        }
        if Self::is_reserved(s) {
            return Err(IdError::Reserved);
        }
        for c in s.chars() {
            if c == '/' || c == '"' || c == '\\' {
                return Err(IdError::BadChar(c));
            }
            let code = c as u32;
            if code < 0x20 || code == 0x7f {
                return Err(IdError::BadChar(c));
            }
        }
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdError::Empty => write!(f, "id is empty"),
            IdError::TooLong { len, max } => write!(f, "id is {len} bytes; max is {max}"),
            IdError::BadChar(c) => write!(f, "id contains forbidden character {c:?}"),
            IdError::LeadingDot => write!(f, "id may not start with '.'"),
            IdError::GitSuffix => write!(f, "id may not end with '.git'"),
            IdError::Reserved => write!(f, "id is reserved by storgit"),
        }
    }
}

impl std::error::Error for IdError {}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Id {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for Id {
    type Err = IdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Id::new(s.to_string())
    }
}

/// A git commit identifier (SHA-1, 20 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(pub [u8; 20]);

impl From<gix::ObjectId> for CommitId {
    fn from(id: gix::ObjectId) -> Self {
        let slice = id.as_slice();
        let mut out = [0u8; 20];
        out.copy_from_slice(&slice[..20]);
        CommitId(out)
    }
}

/// A single historical version of an entry. Every live commit carries
/// a `(label, data)` pair inside the module's tree; a tombstone commit
/// written by [`Store::archive`] has an empty tree and surfaces as
/// `label = None, data = None`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub commit: CommitId,
    pub time: SystemTime,
    pub label: Option<Vec<u8>>,
    pub data: Option<Vec<u8>>,
}

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

/// Map from entry [`Id`] to the bytes of that module's tarball.
pub type Modules = HashMap<Id, Vec<u8>>;

/// Caller-supplied lookup for module bytes. Called by [`Store`] when
/// an operation first touches an id whose bytes are neither on disk
/// nor in [`Parts::modules`]. A fetcher is assumed to reflect live
/// backing storage at call time.
///
/// Return values:
/// - `Ok(Some(bytes))` - module exists, here are its tarball bytes.
/// - `Ok(None)` - no such module in backing storage. If the id is
///   live in the parent's gitlink set, this surfaces as an error
///   (the caller's backing store has diverged from the parent);
///   otherwise the id is treated as fresh.
/// - `Err(e)` - lookup itself failed; the op fails with
///   [`Error::Fetch`].
pub type ModuleFetcher = Arc<
    dyn Fn(&Id) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync + 'static>>
        + Send
        + Sync,
>;

/// The persisted form of a [`Store`].
///
/// Feed this to [`Store::open`] to reconstruct a store. An empty
/// `parent` means "fresh store"; the first [`Store::snapshot`] will
/// emit the newly-initialised state so the caller can persist it.
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
    /// taken, and can be fed straight back into [`Store::open`].
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

/// The delta produced by [`Store::snapshot`]: only the parts touched
/// since the previous snapshot (or, for the first call, since [`Store::open`]).
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

/// Parent bare repo directory inside the scratch dir. Holds the
/// gitlink tree, `.gitmodules`, the `index/` label cache, and its
/// own objects.
const PARENT_DIR: &str = "parent.git";
/// Directory holding per-entry submodule bare repos inside the scratch
/// dir. Each `<id>.git/` is a full bare repo with its own objects.
const MODULES_DIR: &str = "modules";
/// Filenames used within a submodule commit's tree for the payload
/// and the searchable label. Each live commit carries one or both;
/// a tombstone has neither (empty tree).
const DATA_FILE: &str = "data";
const LABEL_FILE: &str = "label";
/// Subtree name inside the parent's root tree under which storgit
/// caches each live module's current label blob, keyed by module id.
/// Modules with empty label have no entry here.
const INDEX_DIR: &str = "index";
/// Filename of the standard git submodule manifest in the parent's
/// root tree. storgit writes this so plain `git` tooling on an
/// extracted tarball recognises the gitlink entries as submodules.
/// Sorts before any valid [`Id`] (leading `.` is forbidden) so it
/// always appears first in the parent tree.
const GITMODULES_FILE: &str = ".gitmodules";
/// Branch that both parent and submodules commit to.
const BRANCH: &str = "refs/heads/main";
/// Identity used for every commit storgit writes. Storgit is a single-
/// writer library; the author is an implementation detail, not metadata
/// the caller can control.
const AUTHOR_NAME: &str = "storgit";
const AUTHOR_EMAIL: &str = "storgit@localhost";

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

/// Relative path within an `objects.git/` directory where git stores
/// a loose object: the first two hex characters of the SHA form a
/// shard directory, the remaining 38 are the filename. `abc1234...`
/// lives at `objects/ab/c1234...`.
fn loose_object_path(objects_dir: &Path, oid: gix::ObjectId) -> PathBuf {
    let hex = oid.to_string();
    let (shard, rest) = hex.split_at(2);
    objects_dir.join("objects").join(shard).join(rest)
}

/// Delete a loose object file if present; not-found is a no-op.
fn drop_loose_object(objects_dir: &Path, oid: gix::ObjectId) -> Result<(), Error> {
    let path = loose_object_path(objects_dir, oid);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Extract a tarball produced by [`tar_dir`] into `dest`. The directory
/// must not already exist; this function creates it.
///
/// `preserve_mtime` and `preserve_permissions` are disabled. Storgit
/// doesn't read either bit on scratch-dir files, so skipping the
/// `utime` / `chmod` syscalls is the correct semantic. `profile_load`
/// suggested a ~20 % load-time win, but a clean criterion before/after
/// showed no measurable difference, so this is for correctness, not
/// speed.
fn untar_into(bytes: &[u8], dest: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(dest)?;
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    archive.set_preserve_mtime(false);
    archive.set_preserve_permissions(false);
    archive.unpack(dest)?;
    Ok(())
}

/// Tar the contents of `dir` into a deterministic uncompressed archive.
fn tar_dir(dir: &Path) -> Result<Vec<u8>, Error> {
    let mut buf = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut buf);
        builder.mode(tar::HeaderMode::Deterministic);
        builder.follow_symlinks(false);
        builder.append_dir_all(".", dir)?;
        builder.finish()?;
    }
    Ok(buf)
}

/// Initialise a full bare repo with its own object database. Used
/// for the parent and for every submodule.
///
/// `gix::init_bare` respects git's `init.defaultBranch` config, so we
/// pin HEAD to [`BRANCH`] afterwards. Also prunes the template junk
/// (`hooks/*.sample`, `info/exclude`, `description`) that would
/// otherwise bloat the tarball.
fn init_bare_on_branch(path: &Path) -> Result<(), Error> {
    gix::init_bare(path)?;
    std::fs::write(path.join("HEAD"), format!("ref: {BRANCH}\n"))?;
    for junk in ["hooks", "info", "description"] {
        let p = path.join(junk);
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else if p.is_file() {
            std::fs::remove_file(&p)?;
        }
    }
    Ok(())
}

/// Write a tombstone commit (empty tree) for a submodule, chaining it
/// onto whatever commit the module's branch currently points at. New
/// objects land in the module's own object DB; the module's
/// `refs/heads/main` file is updated in place.
fn write_tombstone_commit(module: &gix::Repository) -> Result<gix::ObjectId, Error> {
    let module_path = module.path();
    let tree = Tree {
        entries: Vec::new(),
    };
    let tree_id = module.write_object(&tree)?.detach();
    let prior_commit = read_ref_file(&module_ref_path(module_path))?;
    let commit_id = write_commit(
        module,
        tree_id,
        prior_commit.into_iter().collect(),
        "archive",
    )?;
    write_ref_file(&module_ref_path(module_path), commit_id)?;
    Ok(commit_id)
}

/// Write a commit for a submodule whose tree contains up to one blob
/// per slot. For each of [`DATA_FILE`] and [`LABEL_FILE`]: `Some(bytes)`
/// writes a new blob; `None` carries the prior HEAD tree's blob forward
/// (or omits the slot entirely when there is no prior commit). New
/// objects go to the module's own object DB; the module's
/// `refs/heads/main` is updated.
///
/// Returns `Ok(None)` when the newly-built tree is byte-identical to
/// the tree at the module's current HEAD. Git's content-addressing
/// guarantees identical tree bytes produce identical oids, so a single
/// oid equality check against the prior commit's tree is sufficient.
fn write_entry_commit(
    module: &gix::Repository,
    label: Option<&[u8]>,
    data: Option<&[u8]>,
) -> Result<Option<gix::ObjectId>, Error> {
    let module_path = module.path();
    let prior_commit = read_ref_file(&module_ref_path(module_path))?;
    // Only decode the prior tree when we actually need a blob from it.
    let prior_tree_entries = if (label.is_none() || data.is_none())
        && let Some(prior_id) = prior_commit
    {
        let prior = module.find_object(prior_id)?.into_commit();
        let prior_tree_id = prior.decode()?.tree();
        Some(decode_tree(module, prior_tree_id)?.entries)
    } else {
        None
    };
    let prior_blob = |filename: &str| -> Option<gix::ObjectId> {
        prior_tree_entries.as_ref().and_then(|entries| {
            entries
                .iter()
                .find(|e| e.filename.as_bstr() == BStr::new(filename))
                .map(|e| e.oid)
        })
    };

    let mut entries: Vec<TreeEntry> = Vec::with_capacity(2);
    let data_oid = match data {
        Some(bytes) => Some(module.write_blob(bytes)?.detach()),
        None => prior_blob(DATA_FILE),
    };
    if let Some(oid) = data_oid {
        entries.push(TreeEntry {
            mode: EntryKind::Blob.into(),
            filename: BString::from(DATA_FILE),
            oid,
        });
    }
    let label_oid = match label {
        Some(bytes) => Some(module.write_blob(bytes)?.detach()),
        None => prior_blob(LABEL_FILE),
    };
    if let Some(oid) = label_oid {
        entries.push(TreeEntry {
            mode: EntryKind::Blob.into(),
            filename: BString::from(LABEL_FILE),
            oid,
        });
    }
    // DATA_FILE < LABEL_FILE lexicographically, so push order already
    // satisfies git's strict filename sort.
    let tree = Tree { entries };
    let tree_id = module.write_object(&tree)?.detach();

    // No-op detection: if the module already has a HEAD whose tree
    // oid matches the tree we just built, every file (name, mode,
    // contents) is unchanged and we skip writing an identical commit.
    if let Some(prior_id) = prior_commit {
        let prior = module.find_object(prior_id)?.into_commit();
        let prior_tree_id = prior.decode()?.tree();
        if prior_tree_id == tree_id {
            return Ok(None);
        }
    }

    let commit_id = write_commit(module, tree_id, prior_commit.into_iter().collect(), "put")?;
    write_ref_file(&module_ref_path(module_path), commit_id)?;
    Ok(Some(commit_id))
}

/// Path to the `refs/heads/main` file inside a bare repo.
fn module_ref_path(repo_path: &Path) -> PathBuf {
    repo_path.join("refs").join("heads").join("main")
}

/// Read a loose ref file and parse it as an object id. Returns `None`
/// if the file doesn't exist, which means "no commit on this branch yet".
fn read_ref_file(path: &Path) -> Result<Option<gix::ObjectId>, Error> {
    match std::fs::read_to_string(path) {
        Ok(s) => {
            let trimmed = s.trim();
            let oid = gix::ObjectId::from_hex(trimmed.as_bytes())
                .map_err(|e| Error::Other(format!("invalid ref {path:?}: {e}")))?;
            Ok(Some(oid))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Write a loose ref file for `oid`, creating parent directories if
/// needed. Mirrors git's own on-disk format for a branch head: the
/// 40-char hex SHA followed by a newline.
fn write_ref_file(path: &Path, oid: gix::ObjectId) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{oid}\n"))?;
    Ok(())
}

/// Build and write a commit with storgit's canonical author/committer
/// and the current wall-clock time.
fn write_commit(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    parents: Vec<gix::ObjectId>,
    message: &str,
) -> Result<gix::ObjectId, Error> {
    let sig = current_signature();
    let commit = Commit {
        tree,
        parents: parents.into(),
        author: sig.clone(),
        committer: sig,
        encoding: None,
        message: message.into(),
        extra_headers: Vec::new(),
    };
    Ok(repo.write_object(&commit)?.detach())
}

/// Storgit's canonical signature, timestamped to now.
fn current_signature() -> gix::actor::Signature {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    gix::actor::Signature {
        name: AUTHOR_NAME.into(),
        email: AUTHOR_EMAIL.into(),
        time: gix::date::Time { seconds, offset: 0 },
    }
}

/// Seed the in-memory gitlink map and label cache from the persisted
/// parent repo. The gitlink map comes from the root tree's commit-mode
/// entries; the label cache comes from the `index/` subtree's blobs
/// (if present). Returns empty maps when the parent has no HEAD yet
/// (fresh repo).
///
/// Filenames we read back were written by storgit itself from validated
/// [`Id`]s, so [`Id::new`] should succeed. Any failure here means
/// on-disk corruption; surface it as an [`Error::Other`].
type ParentState = (BTreeMap<Id, gix::ObjectId>, BTreeMap<Id, Vec<u8>>);

fn load_parent_state(parent_path: &Path) -> Result<ParentState, Error> {
    let parent = gix::open(parent_path)?;
    let Some(tree) = current_parent_tree(&parent)? else {
        return Ok((BTreeMap::new(), BTreeMap::new()));
    };
    let mut gitlinks = BTreeMap::new();
    let mut index_tree_id: Option<gix::ObjectId> = None;
    for entry in tree.entries {
        match entry.mode.kind() {
            EntryKind::Commit => {
                let id = entry_filename_as_id(&entry.filename)?;
                gitlinks.insert(id, entry.oid);
            }
            EntryKind::Tree if entry.filename.as_bstr() == BStr::new(INDEX_DIR) => {
                index_tree_id = Some(entry.oid);
            }
            _ => {}
        }
    }

    let mut label_cache = BTreeMap::new();
    if let Some(index_tree_id) = index_tree_id {
        let index_tree = decode_tree(&parent, index_tree_id)?;
        for entry in index_tree.entries {
            if matches!(entry.mode.kind(), EntryKind::Blob) {
                let blob = parent.find_object(entry.oid)?;
                let id = entry_filename_as_id(&entry.filename)?;
                label_cache.insert(id, blob.data.clone());
            }
        }
    }
    Ok((gitlinks, label_cache))
}

/// Turn a git tree entry's filename back into a validated [`Id`].
/// Only returns an error when the on-disk state is corrupt (a filename
/// that storgit would never have written).
fn entry_filename_as_id(filename: &BString) -> Result<Id, Error> {
    let s = filename.to_string();
    Id::new(s).map_err(|e| Error::Other(format!("corrupt parent tree entry {filename:?}: {e}")))
}

/// Return the parent repo's current root tree, or `None` if HEAD is
/// unborn (i.e. the parent has never been committed to).
fn current_parent_tree(repo: &gix::Repository) -> Result<Option<Tree>, Error> {
    let Ok(head) = repo.head_commit() else {
        return Ok(None);
    };
    let tree = head.tree()?;
    Ok(Some(decode_tree(repo, tree.id().detach())?))
}

/// Read and decode a tree object by id into an owned [`Tree`].
fn decode_tree(repo: &gix::Repository, id: gix::ObjectId) -> Result<Tree, Error> {
    let object = repo.find_object(id)?;
    let tree = object
        .try_into_tree()
        .map_err(|e| Error::Other(format!("expected tree: {e:?}")))?;
    // `Tree` borrows from `object`'s buffer, but we want an owned copy so
    // we can mutate it. `decode()` returns the borrowed view; we rebuild
    // the owned form directly from that.
    let decoded = tree.decode()?;
    Ok(Tree {
        entries: decoded
            .entries
            .iter()
            .map(|e| TreeEntry {
                mode: e.mode,
                filename: e.filename.into(),
                oid: e.oid.into(),
            })
            .collect(),
    })
}

/// Serialise the parent's gitlink map as a `.gitmodules` config file.
/// One stanza per id; `path` is the gitlink filename in the parent
/// tree (the bare id), `url` is a path relative to the parent repo
/// pointing at the module's bare repo on disk. The relative URL keeps
/// an extracted tarball self-contained: a `git clone --recursive`
/// against the parent finds each submodule next door.
///
/// [`Id`] forbids the only characters that would need escaping in a
/// git-config quoted section name (`"` and `\`), so we can interpolate
/// directly without escaping.
fn serialize_gitmodules(gitlinks: &BTreeMap<Id, gix::ObjectId>) -> Vec<u8> {
    let mut out = String::new();
    for id in gitlinks.keys() {
        let s = id.as_str();
        out.push_str("[submodule \"");
        out.push_str(s);
        out.push_str("\"]\n\tpath = ");
        out.push_str(s);
        out.push_str("\n\turl = ../modules/");
        out.push_str(s);
        out.push_str(".git\n");
    }
    out.into_bytes()
}

/// Build an [`Entry`] for the commit at `commit_id` in `repo`. Reads
/// the commit's time and its tree's `label` / `data` blobs; each slot
/// is `Some(bytes)` if the corresponding file exists in the tree,
/// `None` if absent. Tombstone commits (empty tree) surface as both
/// slots `None`.
fn read_entry_at(repo: &gix::Repository, commit_id: gix::ObjectId) -> Result<Entry, Error> {
    let commit = repo.find_object(commit_id)?.into_commit();
    let time = commit_time(&commit)?;
    let tree_id = commit.decode()?.tree();
    let tree = decode_tree(repo, tree_id)?;
    let mut label = None;
    let mut data = None;
    for entry in tree.entries {
        if entry.filename.as_bstr() == BStr::new(DATA_FILE) {
            data = Some(repo.find_object(entry.oid)?.data.clone());
        } else if entry.filename.as_bstr() == BStr::new(LABEL_FILE) {
            label = Some(repo.find_object(entry.oid)?.data.clone());
        }
    }
    Ok(Entry {
        commit: commit_id.into(),
        time,
        label,
        data,
    })
}

fn commit_time(commit: &gix::Commit<'_>) -> Result<SystemTime, Error> {
    let sig = commit.committer()?;
    let seconds = sig.seconds().max(0) as u64;
    Ok(UNIX_EPOCH + Duration::from_secs(seconds))
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    Git(Box<dyn std::error::Error + Send + Sync + 'static>),
    /// A [`ModuleFetcher`] returned an error.
    Fetch(Box<dyn std::error::Error + Send + Sync + 'static>),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "io: {e}"),
            Error::Git(e) => write!(f, "git: {e}"),
            Error::Fetch(e) => write!(f, "fetch: {e}"),
            Error::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

macro_rules! boxed_from {
    ($($t:ty),+ $(,)?) => {
        $(
            impl From<$t> for Error {
                fn from(e: $t) -> Self { Error::Git(Box::new(e)) }
            }
        )+
    };
}

boxed_from!(
    gix::init::Error,
    gix::open::Error,
    gix::object::find::existing::Error,
    gix::object::write::Error,
    gix::object::commit::Error,
    gix::objs::decode::Error,
    gix::reference::edit::Error,
    gix::revision::walk::Error,
    gix::revision::walk::iter::Error,
    gix::traverse::commit::simple::Error,
);
