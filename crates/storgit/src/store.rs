//! The generic [`Store`] wrapper. Delegates every I/O method to its
//! [`Layout`], and picks up layout-specific methods (like
//! [`Store::<SubmoduleLayout>::open`] / [`Store::<SubmoduleLayout>::snapshot`])
//! via inherent impls in each layout's module.

use std::path::PathBuf;

use crate::entry::{CommitId, Entry};
use crate::error::Error;
use crate::id::Id;
use crate::layout::Layout;
use crate::layout::submodule::SubmoduleLayout;

/// Handle to a storgit store. `L` selects the on-disk layout; the
/// default is [`SubmoduleLayout`], which carries the existing
/// parent-plus-per-entry-submodule persistence model.
pub struct Store<L: Layout = SubmoduleLayout> {
    pub(crate) layout: L,
}

impl<L: Layout> Store<L> {
    /// Initialise a fresh storgit repo at `path` for this layout.
    /// See [`Layout::new`].
    pub fn new(path: PathBuf) -> Result<Self, Error> {
        Ok(Store {
            layout: L::new(path)?,
        })
    }

    /// Open an existing storgit repo at `path` for this layout.
    /// See [`Layout::open`].
    pub fn open(path: PathBuf) -> Result<Self, Error> {
        Ok(Store {
            layout: L::open(path)?,
        })
    }

    /// Bundle the store into a single self-contained tarball.
    /// See [`Layout::save`].
    pub fn save(&mut self) -> Result<Vec<u8>, Error> {
        self.layout.save()
    }

    /// Untar `bytes` into `path` and open the resulting repo.
    /// See [`Layout::load`].
    pub fn load(bytes: &[u8], path: PathBuf) -> Result<Self, Error> {
        Ok(Store {
            layout: L::load(bytes, path)?,
        })
    }

    /// Write a new version of entry `id` whose commit tree carries the
    /// given `label` and/or `data`. See [`Layout::put`] for the full
    /// contract around `None` slots, no-op detection, and the rejection
    /// of `(None, None)`.
    pub fn put(
        &mut self,
        id: &Id,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error> {
        self.layout.put(id, label, data)
    }

    /// Return the latest [`Entry`] for `id`, or `None` if `id` is not
    /// a live entry.
    pub fn get(&self, id: &Id) -> Result<Option<Entry>, Error> {
        self.layout.get(id)
    }

    /// Soft-delete `id` (see [`Layout::archive`]).
    pub fn archive(&mut self, id: &Id) -> Result<(), Error> {
        self.layout.archive(id)
    }

    /// Hard-delete `id` (see [`Layout::delete`]).
    pub fn delete(&mut self, id: &Id) -> Result<(), Error> {
        self.layout.delete(id)
    }

    /// List the ids of all live entries in arbitrary order.
    pub fn list(&self) -> Result<Vec<Id>, Error> {
        self.layout.list()
    }

    /// Walk every historical version of `id`, newest first.
    pub fn history(&self, id: &Id) -> Result<Vec<Entry>, Error> {
        self.layout.history(id)
    }

    /// Return the current label blob for `id`, or `None` if `id` is
    /// not a live entry or its label is absent/empty.
    pub fn label(&self, id: &Id) -> Option<&[u8]> {
        self.layout.label(id)
    }

    /// Return every live entry whose label is non-empty, paired with
    /// that label blob.
    pub fn list_labels(&self) -> Vec<(Id, Vec<u8>)> {
        self.layout.list_labels()
    }
}
