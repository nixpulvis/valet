//! The [`Layout`] trait: the per-operation surface that every
//! concrete storgit layout (submodule, subdir, ...) implements. A
//! [`crate::Store`] is a thin generic wrapper over some `L: Layout`
//! and just delegates each method to the layout.
//!
//! Operations that are specific to one layout (persistence envelopes
//! like [`submodule::Parts`] / [`submodule::Snapshot`], path-based
//! open, etc.) stay as inherent methods on that layout's `Store<L>`
//! rather than living on this trait. The trait is intentionally
//! narrow: the everyday read/write surface, and nothing else.

use std::path::PathBuf;

use crate::entry::{CommitId, Entry};
use crate::error::Error;
use crate::id::Id;
use crate::tarball::untar_into;

pub mod subdir;
pub mod submodule;

pub trait Layout: Sized {
    /// Initialise a fresh storgit repo at `path`. Creates `path` as
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

    fn put(
        &mut self,
        id: &Id,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error>;

    fn get(&self, id: &Id) -> Result<Option<Entry>, Error>;

    fn archive(&mut self, id: &Id) -> Result<(), Error>;

    fn delete(&mut self, id: &Id) -> Result<(), Error>;

    fn list(&self) -> Result<Vec<Id>, Error>;

    fn history(&self, id: &Id) -> Result<Vec<Entry>, Error>;

    fn label(&self, id: &Id) -> Option<&[u8]>;

    fn list_labels(&self) -> Vec<(Id, Vec<u8>)>;
}
