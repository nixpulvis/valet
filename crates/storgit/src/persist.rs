use std::collections::HashMap;
use std::sync::Arc;

use crate::id::Id;

/// Map from entry [`Id`] to the bytes of that module's tarball.
pub type Modules = HashMap<Id, Vec<u8>>;

/// Caller-supplied lookup for module bytes. Called by [`crate::Store`]
/// when an operation first touches an id whose bytes are neither on disk
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
///   [`crate::Error::Fetch`].
pub type ModuleFetcher = Arc<
    dyn Fn(&Id) -> Result<Option<Vec<u8>>, Box<dyn std::error::Error + Send + Sync + 'static>>
        + Send
        + Sync,
>;

/// The persisted form of a [`crate::Store`].
///
/// Feed this to [`crate::Store::open`] to reconstruct a store. An empty
/// `parent` means "fresh store"; the first [`crate::Store::snapshot`]
/// will emit the newly-initialised state so the caller can persist it.
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
    /// taken, and can be fed straight back into [`crate::Store::open`].
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

/// The delta produced by [`crate::Store::snapshot`]: only the parts
/// touched since the previous snapshot (or, for the first call, since
/// [`crate::Store::open`]).
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
