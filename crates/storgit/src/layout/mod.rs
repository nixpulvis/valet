//! On-disk shapes that back a [`Store`](crate::Store).
//!
//! The [`Layout`] trait is the surface API that every concrete storgit layout
//! (submodule, subdir) implements. A [`Store`] is a thin generic wrapper over
//! a layout and just delegates each method to the layout. Available layouts
//! are:
//!
//! - [`SubdirLayout`]: a single bare repo at `<path>/` whose tree carries all
//! entries as `<path>/records/<id>/{data,label}`, with one shared ref and
//! per-entry history via path-scoped walks.
//! - [`SubmoduleLayout`]: a `<path>/parent.git/` bare repo whose tree carries
//! one _gitlink_ per live entry plus a `.gitmodules` manifest, alongside
//! `<path>/modules/<id>/{data,label}` bare repos each holding that entry's
//! `{data,label}` blobs in their own object database.
//!
//! Operations that are specific to one layout (persistence envelopes like
//! [`submodule::Parts`] / [`submodule::Snapshot`], etc.) stay as inherent
//! methods on that layout's `Store<L>` rather than living on this trait.
//!
//! [`Store`]: crate::Store
//! [`SubdirLayout`]: crate::SubdirLayout
//! [`SubmoduleLayout`]: crate::SubmoduleLayout

use std::path::PathBuf;

use crate::entry::{CommitId, Entry};
use crate::error::Error;
use crate::id::Id;
use crate::tarball::untar_into;

pub mod subdir;
pub mod submodule;

pub trait Layout: Sized {
    /// Initialize a fresh storgit repo at `path`. Creates `path` as
    /// a new directory; errors if `path` already exists. The parent
    /// directory must already exist -- `new` does not create
    /// ancestors.
    fn new(path: PathBuf) -> Result<Self, Error>;

    /// Open an existing storgit repo at `path`. Errors if `path`
    /// does not exist or is not a valid storgit repo for this layout.
    fn open(path: PathBuf) -> Result<Self, Error>;

    /// Bundle the store into a single self-contained tarball. See
    /// per-layout docs for what "self-contained" means (e.g.
    /// submodule force-loads every live module before tarring).
    fn save(&mut self) -> Result<Vec<u8>, Error>;

    /// Untar `bytes` into `path`, then open the result. `path` must
    /// not exist or must be empty. Default impl works for any layout
    /// whose on-disk shape is what `save` produced.
    fn load(bytes: &[u8], path: PathBuf) -> Result<Self, Error> {
        if path.exists() && path.read_dir()?.next().is_some() {
            return Err(Error::Other(format!(
                "load: target path {path:?} is not empty"
            )));
        }
        untar_into(bytes, &path)?;
        Self::open(path)
    }

    /// Write a new version of entry `id` carrying the given `label`
    /// and/or `data` blobs.
    ///
    /// At least one of `label` or `data` must be `Some`; passing
    /// `(None, None)` returns an error. Use [`Layout::archive`] to
    /// record a tombstone.
    ///
    /// Slot semantics:
    /// - `Some(bytes)` replaces that slot with `bytes` (empty slice
    ///   is allowed and is distinct from absent).
    /// - `None` carries the prior commit's blob forward unchanged;
    ///   on a fresh entry, the slot is simply omitted.
    ///
    /// No-op detection: if the resulting per-entry tree is identical
    /// to the prior one, no commit is written and `Ok(None)` is
    /// returned. Otherwise returns the new [`CommitId`].
    fn put(
        &mut self,
        id: &Id,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error>;

    /// Return the latest [`Entry`] for `id`, or `None` if `id` is not
    /// currently live (never written, archived, or deleted).
    fn get(&self, id: &Id) -> Result<Option<Entry>, Error>;

    /// Soft-delete `id`: record a tombstone commit so the entry stops
    /// appearing in [`Layout::list`] / [`Layout::get`], but its prior
    /// versions remain reachable via [`Layout::history`]. Archiving
    /// an unknown or already-archived id is a no-op.
    fn archive(&mut self, id: &Id) -> Result<(), Error>;

    /// Hard-delete `id`: remove the entry and, where the layout
    /// supports it, its history as well. Layouts that cannot cheaply
    /// erase per-entry history may fall back to the same behaviour
    /// as [`Layout::archive`]; see the per-layout docs. Deleting an
    /// unknown id is a no-op.
    fn delete(&mut self, id: &Id) -> Result<(), Error>;

    /// List the ids of all live entries, in arbitrary order.
    /// Archived and deleted ids are excluded.
    fn list(&self) -> Result<Vec<Id>, Error>;

    /// Walk every historical version of `id`, newest first. Includes
    /// the tombstone commit for archived entries. Returns an empty
    /// vec if `id` has no history in this store.
    fn history(&self, id: &Id) -> Result<Vec<Entry>, Error>;

    /// Return the current label blob for `id`, or `None` if `id` is
    /// not live or its label slot is absent/empty. Served from an
    /// in-memory cache, so this is cheap and does not hit the repo.
    fn label(&self, id: &Id) -> Option<&[u8]>;

    /// Return every live entry whose label slot is non-empty, paired
    /// with that label blob. Served from the same in-memory cache as
    /// [`Layout::label`].
    fn list_labels(&self) -> Vec<(Id, Vec<u8>)>;
}
