//! [`Store`] = thin generic wrapper over a [`Layout`].
//!
//! Every operation lives on the layout itself, reachable through
//! `Store`'s `Deref` impl: record I/O ([`Layout`]), the
//! resolve-and-finalise merge primitives ([`crate::Merge`]), the
//! remote configuration + fetch/push/pull surface
//! ([`crate::Distribute`]), and the layout-specific bundle envelopes.
//! `Store` exists today only to host the `new` / `open` / `load`
//! constructors; once the trait surface stops moving the wrapper
//! goes away and these constructors land directly on each layout.

use std::ops::{Deref, DerefMut};
use std::path::PathBuf;

use crate::error::Error;
use crate::layout::Layout;
use crate::layout::submodule::SubmoduleLayout;

/// Git backed database with a specific layout.
///
/// The [`Layout`] selects the on-disk layout, which defaults to
/// [`SubmoduleLayout`].
pub struct Store<L: Layout = SubmoduleLayout> {
    pub layout: L,
}

impl<L: Layout> Deref for Store<L> {
    type Target = L;
    fn deref(&self) -> &L {
        &self.layout
    }
}

impl<L: Layout> DerefMut for Store<L> {
    fn deref_mut(&mut self) -> &mut L {
        &mut self.layout
    }
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

    /// Snap-decompress `bytes`, untar into `path`, and open the
    /// resulting repo. See [`Layout::load`].
    pub fn load(bytes: &[u8], path: PathBuf) -> Result<Self, Error> {
        Ok(Store {
            layout: L::load(bytes, path)?,
        })
    }
}
