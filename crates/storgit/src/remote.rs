//! [`Remote`], the [`Distribute`] trait that owns every operation
//! touching the network, and the fetch primitives that drive a
//! `gix::Remote` to move objects.
//!
//! Remotes live in the layout's [git_dir](crate::Layout::git_dir) under
//! standard `[remote "<name>"]` sections (parsed by
//! [`crate::config::GitConfig`]), so `gix::remote` and every other git
//! tool see them transparently.

use std::path::Path;
use std::process::Command;

use gix::bstr::ByteSlice;

use crate::Layout;
use crate::error::Error;
use crate::git::{BRANCH, init_bare_on_branch};
use crate::merge::MergeStatus;

/// A named remote: a name and a URL. Pure data; the I/O lives on
/// the [`Distribute`] trait.
///
/// Constructed by reading the bare repo's git config; callers reach
/// `Remote` instances through [`Distribute::remotes`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// Every operation that distributes a layout to or from a remote
/// peer: configuration setup, fetch, push, and pull. Layout-bound
/// because it reads/writes the layout's git config and fetches into
/// the layout's bare repo.
///
/// Setup (`add_remote`, `remove_remote`, `remotes`) and the basic
/// `fetch` / `push` operations have default impls that work for any
/// layout. `pull` is layout-specific (submodule fetches the parent
/// then recurses into each changed module) and is required.
pub trait Distribute: Layout {
    /// Configure a named remote pointing at `url`. Stored as a
    /// standard `[remote "<name>"]` entry in the layout's git
    /// config, visible to `gix::remote` and every other git tool.
    /// Errors if `name` already exists or is invalid.
    fn add_remote(&mut self, name: &str, url: &str) -> Result<(), Error> {
        crate::config::GitConfig::add_remote(&self.git_dir(), name, url)
    }

    /// Remove a previously-configured remote. Errors if no such
    /// remote is configured.
    fn remove_remote(&mut self, name: &str) -> Result<(), Error> {
        crate::config::GitConfig::remove_remote(&self.git_dir(), name)
    }

    /// Every configured remote, as [`Remote`] values.
    fn remotes(&self) -> Result<Vec<Remote>, Error> {
        crate::config::GitConfig::list_remotes(&self.git_dir())
    }

    /// Fetch from the named remote into the local object database.
    /// Updates `refs/remotes/<name>/*`; does not touch local HEAD
    /// or any local branch. Errors if `remote` is not configured.
    ///
    /// For the submodule layout, this fetches only the parent repo;
    /// per-module fetches happen inside [`Self::pull`] once the
    /// parent's incoming gitlinks are known.
    fn fetch(&mut self, remote: &str) -> Result<(), Error> {
        // Confirm the remote is configured (clearer error than gix's).
        crate::config::GitConfig::lookup_remote(&self.git_dir(), remote)?;
        let repo = gix::open(self.git_dir())?;
        let remote_obj = repo
            .find_remote(remote)
            .map_err(|e| Error::Other(format!("fetch: remote {remote:?}: {e}")))?;
        do_fetch(remote_obj)
    }

    /// Fetch from `remote` and merge its branch into the local
    /// store. A remote with no branch yet is a clean no-op.
    fn pull(&mut self, remote: &str) -> Result<MergeStatus, Error>;

    /// Push the canonical [`BRANCH`] of the layout's bare repo to
    /// `remote`. Default impl: shell out to the system `git` binary,
    /// since `gix` 0.81 does not yet ship a push transport. Layouts
    /// that span multiple bare repos (e.g. submodule's parent +
    /// per-id modules) override this to push each.
    ///
    /// For `file://` URLs whose target directory does not exist, the
    /// directory is initialised as a bare repo first. Other schemes
    /// (`ssh://`, `https://`) are passed through to git unchanged;
    /// the remote endpoint must exist already.
    fn push(&self, remote: &str) -> Result<(), Error> {
        let url = crate::config::GitConfig::lookup_remote(&self.git_dir(), remote)?.url;
        shell_push(&self.git_dir(), &url)
    }
}

/// Push the canonical [`BRANCH`] of the bare repo at `repo_dir` to
/// `url` by shelling out to `git push`. Auto-initialises a bare repo
/// at the destination for `file://` URLs whose path does not yet
/// exist, so a first push from a fresh remote succeeds without an
/// out-of-band `git init --bare` step.
pub(crate) fn shell_push(repo_dir: &Path, url: &str) -> Result<(), Error> {
    if let Some(local) = url.strip_prefix("file://") {
        let local_path = Path::new(local);
        if !local_path.exists() {
            if let Some(parent) = local_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            init_bare_on_branch(local_path)?;
        }
    }
    let refspec = format!("{BRANCH}:{BRANCH}");
    let output = Command::new("git")
        .arg("push")
        .arg(url)
        .arg(&refspec)
        .current_dir(repo_dir)
        .output()
        .map_err(|e| Error::Other(format!("git push: spawn failed: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(Error::PushRejected {
            remote: url.to_string(),
            reason: if stderr.is_empty() {
                format!("git push exited with {}", output.status)
            } else {
                stderr
            },
        });
    }
    Ok(())
}

/// Drive a configured `gix::Remote` through connect -> prepare ->
/// receive with storgit's canonical no-progress, non-interruptible
/// settings. Shared by [`fetch_into`] (ad-hoc URL) and
/// [`Distribute::fetch`] (configured remote by name).
pub(crate) fn do_fetch(remote: gix::Remote<'_>) -> Result<(), Error> {
    use gix::remote::Direction;
    use std::sync::atomic::AtomicBool;

    let connection = remote
        .connect(Direction::Fetch)
        .map_err(|e| Error::Git(Box::new(e)))?;
    let prepare = connection
        .prepare_fetch(gix::progress::Discard, Default::default())
        .map_err(|e| Error::Git(Box::new(e)))?;
    prepare
        .receive(gix::progress::Discard, &AtomicBool::new(false))
        .map_err(|e| Error::Git(Box::new(e)))?;
    Ok(())
}

/// Fetch `refs/heads/main` from `url` into the bare repo at
/// `repo_path`, landing it at `refs/storgit/incoming`. Used by the
/// submodule merge kernel for one-shot fetches against an ad-hoc URL
/// (a per-module URL derived from the parent's URL) without
/// registering the URL as a configured remote.
///
/// **Does not touch local `refs/heads/main`.** The incoming oid is
/// only addressable via `refs/storgit/incoming` afterwards and via
/// the object DB; the merge kernel works on oids directly and
/// updates `refs/heads/main` itself when it advances a gitlink.
/// Force-writing local `refs/heads/main` here would orphan any
/// commits the local side produced after its prior pull, causing
/// the next push to send an outdated ref alongside a parent that
/// references newer gitlink oids the remote never received.
pub(crate) fn fetch_into(repo_path: &Path, url: &str) -> Result<(), Error> {
    use gix::remote::Direction;

    let repo = gix::open(repo_path)?;
    let parsed_url = gix::url::Url::try_from(url)
        .map_err(|e| Error::Other(format!("invalid url {url:?}: {e}")))?;
    let remote = repo
        .remote_at(parsed_url)
        .map_err(|e| Error::Git(Box::new(e)))?
        .with_refspecs(
            [b"+refs/heads/main:refs/storgit/incoming".as_bstr()],
            Direction::Fetch,
        )
        .map_err(|e| Error::Git(Box::new(e)))?;
    do_fetch(remote)
}

#[cfg(test)]
mod tests {
    use crate::config::GitConfig;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let git_dir = dir.path().to_path_buf();
        fs::write(
            GitConfig::path(&git_dir),
            "[core]\n\tbare = true\n\trepositoryformatversion = 0\n",
        )
        .unwrap();
        (dir, git_dir)
    }

    #[test]
    fn list_empty_when_no_remotes() {
        let (_d, g) = fixture();
        assert!(GitConfig::list_remotes(&g).unwrap().is_empty());
    }

    #[test]
    fn add_then_list_roundtrips() {
        let (_d, g) = fixture();
        GitConfig::add_remote(&g, "origin", "https://example.com/repo.git").unwrap();
        let remotes = GitConfig::list_remotes(&g).unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "https://example.com/repo.git");
    }

    #[test]
    fn add_duplicate_name_errors() {
        let (_d, g) = fixture();
        GitConfig::add_remote(&g, "origin", "url1").unwrap();
        assert!(GitConfig::add_remote(&g, "origin", "url2").is_err());
    }

    #[test]
    fn remove_removes_entry() {
        let (_d, g) = fixture();
        GitConfig::add_remote(&g, "origin", "url").unwrap();
        GitConfig::remove_remote(&g, "origin").unwrap();
        assert!(GitConfig::list_remotes(&g).unwrap().is_empty());
    }

    #[test]
    fn remove_unknown_errors() {
        let (_d, g) = fixture();
        assert!(GitConfig::remove_remote(&g, "origin").is_err());
    }

    #[test]
    fn preserves_unrelated_sections() {
        let (_d, g) = fixture();
        GitConfig::add_remote(&g, "origin", "url").unwrap();
        GitConfig::remove_remote(&g, "origin").unwrap();
        let text = fs::read_to_string(GitConfig::path(&g)).unwrap();
        assert!(text.contains("[core]"));
        assert!(text.contains("bare = true"));
    }

    #[test]
    fn multiple_remotes_coexist() {
        let (_d, g) = fixture();
        GitConfig::add_remote(&g, "a", "url-a").unwrap();
        GitConfig::add_remote(&g, "b", "url-b").unwrap();
        let remotes = GitConfig::list_remotes(&g).unwrap();
        assert_eq!(remotes.len(), 2);
        let names: Vec<_> = remotes.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn rejects_name_with_quote() {
        let (_d, g) = fixture();
        assert!(GitConfig::add_remote(&g, "bad\"name", "url").is_err());
    }

    #[test]
    fn list_missing_config_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(GitConfig::list_remotes(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn lookup_returns_remote() {
        let (_d, g) = fixture();
        GitConfig::add_remote(&g, "origin", "url").unwrap();
        let r = GitConfig::lookup_remote(&g, "origin").unwrap();
        assert_eq!(r.name, "origin");
        assert_eq!(r.url, "url");
    }

    #[test]
    fn lookup_unknown_errors() {
        let (_d, g) = fixture();
        assert!(GitConfig::lookup_remote(&g, "origin").is_err());
    }
}
