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

    /// Path to the bare git repository that owns this store's
    /// remote configuration and sync surface. See
    /// [`Layout::git_dir`].
    pub fn git_dir(&self) -> PathBuf {
        self.layout.git_dir()
    }

    /// Configure a named remote pointing at `url`. Stored as a
    /// standard `[remote "<name>"]` entry in the layout's git
    /// config, visible to `gix::remote` and every other git tool.
    /// Errors if `name` already exists or is invalid.
    pub fn add_remote(&mut self, name: &str, url: &str) -> Result<(), Error> {
        crate::remote::Remotes::new(&self.layout.git_dir()).add(name, url)
    }

    /// Remove a previously-configured remote. Errors if no such
    /// remote is configured.
    pub fn remove_remote(&mut self, name: &str) -> Result<(), Error> {
        crate::remote::Remotes::new(&self.layout.git_dir()).remove(name)
    }

    /// Iterate the (name, url) of every configured remote.
    pub fn remotes(&self) -> Result<Vec<(String, String)>, Error> {
        Ok(crate::remote::Remotes::new(&self.layout.git_dir())
            .list()?
            .into_iter()
            .map(|r| (r.name, r.url))
            .collect())
    }

    /// Push local refs to a configured remote. Currently
    /// unimplemented: `gix` 0.81 does not yet ship a push
    /// transport, and storgit does not shell out. Callers
    /// needing one-way replication can use `snapshot`/`apply`
    /// or `save`/`load` over any pipe in the meantime.
    pub fn push(&self, remote: &str) -> Result<(), Error> {
        let remotes = self.remotes()?;
        if !remotes.iter().any(|(n, _)| n == remote) {
            return Err(Error::Other(format!("push: remote {remote:?} not found")));
        }
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
        let repo = gix::open(self.layout.git_dir())?;
        let remote_obj = repo
            .find_remote(remote)
            .map_err(|e| Error::Other(format!("fetch: remote {remote:?} not found: {e}")))?;
        crate::remote::do_fetch(remote_obj)
    }
}

/// Generic delegation for every layout that implements
/// [`MergeKernel`]. Lets callers write `store.pull(...)`,
/// `store.merge(...)`, etc. without knowing which layout they have.
impl<L: crate::merge::MergeKernel> Store<L> {
    /// See [`MergeKernel::merge_in_progress`].
    pub fn merge_in_progress(&self) -> bool {
        L::merge_in_progress(self)
    }

    /// See [`MergeKernel::abort`].
    pub fn abort(&mut self) -> Result<(), Error> {
        L::abort(self)
    }

    /// See [`MergeKernel::merge`].
    pub fn merge(
        &mut self,
        resolution: crate::merge::MergeResolution<L>,
    ) -> Result<L::Advanced, Error> {
        L::merge(self, resolution)
    }

    /// See [`MergeKernel::pull`].
    pub fn pull(&mut self, remote: &str) -> Result<crate::merge::MergeStatus<L>, Error> {
        L::pull(self, remote)
    }
}
