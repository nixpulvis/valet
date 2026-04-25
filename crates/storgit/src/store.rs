//! [`Store`] = [`Layout`] (record I/O on disk) plus the remote
//! configuration shared across both layouts.
//!
//! Record-level operations and the merge primitives that bracket a
//! pull's lifecycle (`merge_in_progress`, `abort`, `merge`, `pull`)
//! live on the layout itself: `store.layout.put(...)`,
//! `store.layout.snapshot()`, `store.layout.pull(remote)`,
//! `store.layout.merge(resolution)`. Remote configuration management
//! (`add_remote` / `remove_remote` / `remotes` / `fetch` / `push`)
//! lives here, because a remote is a single concept that applies the
//! same way to subdir's repo and to submodule's parent (with the
//! per-module URLs derived from the parent's remote at fetch time).

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

    /// Configure a named remote pointing at `url`. Stored as a
    /// standard `[remote "<name>"]` entry in the layout's git
    /// config, visible to `gix::remote` and every other git tool.
    /// Errors if `name` already exists or is invalid.
    pub fn add_remote(&mut self, name: &str, url: &str) -> Result<(), Error> {
        crate::config::GitConfig::add_remote(&self.layout.git_dir(), name, url)
    }

    /// Remove a previously-configured remote. Errors if no such
    /// remote is configured.
    pub fn remove_remote(&mut self, name: &str) -> Result<(), Error> {
        crate::config::GitConfig::remove_remote(&self.layout.git_dir(), name)
    }

    /// Every configured remote, as [`Remote`](crate::Remote) values.
    pub fn remotes(&self) -> Result<Vec<crate::Remote>, Error> {
        crate::config::GitConfig::list_remotes(&self.layout.git_dir())
    }

    /// Push local refs to a configured remote. Currently
    /// unimplemented: `gix` 0.81 does not yet ship a push
    /// transport, and storgit does not shell out. Callers
    /// needing one-way replication can use `snapshot`/`apply`
    /// or `save`/`load` over any pipe in the meantime.
    pub fn push(&self, remote: &str) -> Result<(), Error> {
        crate::config::GitConfig::lookup_remote(&self.layout.git_dir(), remote)?;
        Err(Error::PushRejected {
            remote: remote.to_string(),
            reason: "push transport not yet supported (gix 0.81 lacks \
                     push); use snapshot/apply or save/load over a \
                     non-git pipe instead"
                .to_string(),
        })
    }

    /// Fetch from the named remote into the local object database.
    /// Updates `refs/remotes/<name>/*`; does not touch local HEAD
    /// or any local branch. Errors if `remote` is not configured.
    pub fn fetch(&mut self, remote: &str) -> Result<(), Error> {
        // Confirm the remote is configured (clearer error than gix's).
        crate::config::GitConfig::lookup_remote(&self.layout.git_dir(), remote)?;
        let repo = gix::open(self.layout.git_dir())?;
        let remote_obj = repo
            .find_remote(remote)
            .map_err(|e| Error::Other(format!("fetch: remote {remote:?}: {e}")))?;
        crate::remote::do_fetch(remote_obj)
    }
}
