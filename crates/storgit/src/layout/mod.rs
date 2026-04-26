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
//! Each layout publishes a per-layout `Bundle` (see [`Layout::Bundle`]),
//! the self-contained transferable object graph + refs that
//! [`Layout::apply`] consumes and [`Layout::bundle`] produces.
//!
//! [`Store`]: crate::Store
//! [`SubdirLayout`]: crate::SubdirLayout
//! [`SubmoduleLayout`]: crate::SubmoduleLayout

use std::io;
use std::path::PathBuf;

use crate::entry::Entry;
use crate::error::Error;
use crate::id::CommitId;
use crate::id::EntryId;
use crate::merge::{ApplyMode, MergeStatus};
use crate::tarball::untar_into;

pub mod subdir;
pub mod submodule;

/// Hash function the bare repos under any storgit layout use for
/// object ids. SHA-1 today; revisit if/when storgit learns to init
/// repos with SHA-256.
pub(crate) const HASH_TYPE: gix::hash::Kind = gix::hash::Kind::Sha1;

pub trait Layout: Sized {
    /// Self-contained transferable form of this layout's persisted
    /// state: object graph + refs, in whatever shape the layout
    /// finds natural (submodule: parent + per-module tarballs;
    /// subdir: the single repo's tarball). Pairs with
    /// [`Layout::apply`] (consumer) and [`Layout::bundle`] (producer).
    type Bundle;

    /// Re-bundle everything touched since the previous
    /// [`bundle`](Self::bundle) call (or since [`open`](Self::open)
    /// for the first call) and hand the caller exactly the parts
    /// that need repersisting. Clears dirty tracking on success, so
    /// back-to-back calls with no intervening writes return an empty
    /// bundle.
    fn bundle(&mut self) -> Result<Self::Bundle, Error>;

    /// Fold `bundle` into this layout, merging or fast-forwarding
    /// per `mode`. Returns [`MergeStatus::Clean`] when the merge is
    /// finalised, [`MergeStatus::Conflicted`] when the caller needs
    /// to drive a [`MergeProgress`](crate::MergeProgress) resolution.
    /// Errors with [`Error::NotFastForward`] under
    /// [`ApplyMode::FastForwardOnly`] when the bundle is divergent.
    fn apply(&mut self, bundle: Self::Bundle, mode: ApplyMode) -> Result<MergeStatus, Error>;

    /// Path to the bare git repository that owns this store's
    /// remote configuration and sync surface. Submodule: the
    /// parent repo. Subdir: the store's single repo.
    fn git_dir(&self) -> PathBuf;

    /// Initialize a fresh storgit repo at `path`. Creates `path` as
    /// a new directory; errors if `path` already exists. The parent
    /// directory must already exist -- `new` does not create
    /// ancestors.
    fn new(path: PathBuf) -> Result<Self, Error>;

    /// Open an existing storgit repo at `path`. Errors if `path`
    /// does not exist or is not a valid storgit repo for this layout.
    fn open(path: PathBuf) -> Result<Self, Error>;

    /// Raw tarball of the on-disk repo state. See per-layout docs
    /// for what "self-contained" means (e.g. submodule force-loads
    /// every live module before tarring).
    fn save_tar(&mut self) -> Result<Vec<u8>, Error>;

    /// Untar `bytes` into `path`, then open the result. `path` must
    /// not exist or must be empty. Default impl works for any layout
    /// whose on-disk shape is what `save_tar` produced.
    fn load_tar(bytes: &[u8], path: PathBuf) -> Result<Self, Error> {
        if path.exists() && path.read_dir()?.next().is_some() {
            return Err(Error::Other(format!(
                "load: target path {path:?} is not empty"
            )));
        }
        untar_into(bytes, &path)?;
        Self::open(path)
    }

    /// Snap-compressed self-contained bundle of the on-disk repo
    /// state. Pairs with [`load`](Self::load) over any non-git pipe.
    fn save(&mut self) -> Result<Vec<u8>, Error> {
        let tarball = self.save_tar()?;
        let mut compressed = Vec::new();
        let mut encoder = snap::read::FrameEncoder::new(tarball.as_slice());
        io::copy(&mut encoder, &mut compressed)?;
        Ok(compressed)
    }

    /// Snap-decompress `bytes`, untar into `path`, and open the
    /// resulting repo. Inverse of [`save`](Self::save).
    fn load(bytes: &[u8], path: PathBuf) -> Result<Self, Error> {
        let mut tarball = Vec::new();
        let mut decoder = snap::read::FrameDecoder::new(bytes);
        io::copy(&mut decoder, &mut tarball)?;
        Self::load_tar(&tarball, path)
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
        id: &EntryId,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error>;

    /// Return the latest [`Entry`] for `id`, or `None` if `id` is not
    /// currently live (never written, archived, or deleted).
    fn get(&self, id: &EntryId) -> Result<Option<Entry>, Error>;

    /// Soft-delete `id`: record a tombstone commit so the entry stops
    /// appearing in [`Layout::list`] / [`Layout::get`], but its prior
    /// versions remain reachable via [`Layout::history`]. Archiving
    /// an unknown or already-archived id is a no-op.
    ///
    /// Returns `true` when a tombstone was actually written, `false`
    /// when the call was a no-op (id wasn't live). Lets callers
    /// distinguish "I just archived this" from "nothing to do."
    fn archive(&mut self, id: &EntryId) -> Result<bool, Error>;

    /// Hard-delete `id`: remove the entry and, where the layout
    /// supports it, its history as well. Layouts that cannot cheaply
    /// erase per-entry history may fall back to the same behaviour
    /// as [`Layout::archive`]; see the per-layout docs. Deleting an
    /// unknown id is a no-op.
    fn delete(&mut self, id: &EntryId) -> Result<(), Error>;

    /// List the ids of all live entries, in arbitrary order.
    /// Archived and deleted ids are excluded.
    fn list(&self) -> Result<Vec<EntryId>, Error>;

    /// Walk every historical version of `id`, newest first. Includes
    /// the tombstone commit for archived entries. Returns an empty
    /// vec if `id` has no history in this store.
    fn history(&self, id: &EntryId) -> Result<Vec<Entry>, Error>;

    /// Return the current label blob for `id`, or `None` if `id` is
    /// not live or its label slot is absent/empty. Served from an
    /// in-memory cache, so this is cheap and does not hit the repo.
    fn label(&self, id: &EntryId) -> Option<&[u8]>;

    /// Return every live entry whose label slot is non-empty, paired
    /// with that label blob. Served from the same in-memory cache as
    /// [`Layout::label`].
    fn list_labels(&self) -> Vec<(EntryId, Vec<u8>)>;
}
