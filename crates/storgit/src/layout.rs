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

use crate::entry::{CommitId, Entry};
use crate::error::Error;
use crate::id::Id;

pub mod submodule;

pub trait Layout {
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
