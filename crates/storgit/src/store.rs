//! The generic [`Store`] wrapper. Delegates every I/O method to its
//! [`Layout`], and picks up layout-specific methods (like
//! [`Store::<SubmoduleLayout>::open`] / [`Store::<SubmoduleLayout>::snapshot`])
//! via inherent impls in each layout's module.

use std::io;
use std::path::PathBuf;

use crate::entry::Entry;
use crate::error::Error;
use crate::id::CommitId;
use crate::id::EntryId;
use crate::layout::Layout;
use crate::layout::submodule::SubmoduleLayout;

/// Git backed database with a specific layout.
///
/// The [`Layout`] selects the on-disk layout, which defaults to
/// [`SubmoduleLayout`].
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

    /// Bundle the store into a single self-contained snap-compressed
    /// tarball. See [`Layout::save`].
    pub fn save(&mut self) -> Result<Vec<u8>, Error> {
        let tarball = self.layout.save()?;
        let mut compressed = Vec::new();
        let mut encoder = snap::read::FrameEncoder::new(tarball.as_slice());
        io::copy(&mut encoder, &mut compressed)?;
        Ok(compressed)
    }

    /// Snap-decompress `bytes`, untar into `path`, and open the
    /// resulting repo. See [`Layout::load`].
    pub fn load(bytes: &[u8], path: PathBuf) -> Result<Self, Error> {
        let mut tarball = Vec::new();
        let mut decoder = snap::read::FrameDecoder::new(bytes);
        io::copy(&mut decoder, &mut tarball)?;
        Ok(Store {
            layout: L::load(&tarball, path)?,
        })
    }

    /// Write a new version of entry `id` with the given `label` and/or
    /// `data` slots. See [`Layout::put`].
    pub fn put(
        &mut self,
        id: &EntryId,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error> {
        self.layout.put(id, label, data)
    }

    /// Return the latest [`Entry`] for `id`. See [`Layout::get`].
    pub fn get(&self, id: &EntryId) -> Result<Option<Entry>, Error> {
        self.layout.get(id)
    }

    /// Soft-delete `id`. See [`Layout::archive`].
    pub fn archive(&mut self, id: &EntryId) -> Result<(), Error> {
        self.layout.archive(id)
    }

    /// Hard-delete `id`. See [`Layout::delete`].
    pub fn delete(&mut self, id: &EntryId) -> Result<(), Error> {
        self.layout.delete(id)
    }

    /// List the ids of all live entries. See [`Layout::list`].
    pub fn list(&self) -> Result<Vec<EntryId>, Error> {
        self.layout.list()
    }

    /// Walk every historical version of `id`, newest first.
    /// See [`Layout::history`].
    pub fn history(&self, id: &EntryId) -> Result<Vec<Entry>, Error> {
        self.layout.history(id)
    }

    /// Return the current label blob for `id`, if any.
    /// See [`Layout::label`].
    pub fn label(&self, id: &EntryId) -> Option<&[u8]> {
        self.layout.label(id)
    }

    /// Return every live entry with a non-empty label.
    /// See [`Layout::list_labels`].
    pub fn list_labels(&self) -> Vec<(EntryId, Vec<u8>)> {
        self.layout.list_labels()
    }
}
