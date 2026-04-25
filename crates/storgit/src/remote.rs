//! Git-config remote management and fetch plumbing.
//!
//! Remotes live in the layout's [git_dir](crate::layout::Layout::git_dir)
//! under standard `[remote "<name>"]` sections (parsed by
//! [`crate::config::GitConfig`]), so `gix::remote` and every other git
//! tool see them transparently.

use std::path::Path;

use gix::bstr::ByteSlice;

use crate::config::GitConfig;
use crate::error::Error;

pub use crate::config::RemoteEntry;

/// Path-scoped handle on a bare repo's `[remote "<name>"]` config
/// sections. Every op reads and writes `git_dir/config` through
/// [`GitConfig`]; callers pass the git-dir in once and then chain
/// method calls instead of threading it through every free fn.
#[derive(Clone, Copy)]
pub(crate) struct Remotes<'a> {
    git_dir: &'a Path,
}

impl<'a> Remotes<'a> {
    pub(crate) fn new(git_dir: &'a Path) -> Self {
        Self { git_dir }
    }

    /// List all remote entries in `git_dir`'s config.
    pub(crate) fn list(&self) -> Result<Vec<RemoteEntry>, Error> {
        Ok(GitConfig::read(self.git_dir)?.remotes().collect())
    }

    /// Add a `[remote "<name>"]` section with the given URL. Errors
    /// if a remote with that name already exists.
    pub(crate) fn add(&self, name: &str, url: &str) -> Result<(), Error> {
        validate_remote_name(name)?;
        let mut config = GitConfig::read(self.git_dir)?;
        if config.has_remote(name) {
            return Err(Error::Other(format!("remote {name:?} already exists")));
        }
        config.add_remote(name, url);
        config.write(self.git_dir)
    }

    /// Remove the `[remote "<name>"]` section. Errors if no such
    /// remote exists.
    pub(crate) fn remove(&self, name: &str) -> Result<(), Error> {
        let mut config = GitConfig::read(self.git_dir)?;
        if config.remove_remote(name) == 0 {
            return Err(Error::Other(format!("remote {name:?} not found")));
        }
        config.write(self.git_dir)
    }

    /// Look up the URL of the remote named `name`. Errors if the
    /// remote is not configured.
    pub(crate) fn lookup_url(&self, name: &str) -> Result<String, Error> {
        GitConfig::read(self.git_dir)?
            .remotes()
            .find(|r| r.name == name)
            .map(|r| r.url)
            .ok_or_else(|| Error::Other(format!("remote {name:?} not found")))
    }
}

/// Drive a configured `gix::Remote` through connect -> prepare ->
/// receive with storgit's canonical no-progress, non-interruptible
/// settings. Shared by [`fetch_into`] (ad-hoc URL) and
/// [`crate::Store::fetch`] (configured remote by name).
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
/// `repo_path`, updating its local `refs/heads/main` to match. Used
/// by the merge kernel for one-shot fetches against an ad-hoc URL
/// (e.g. a per-submodule URL derived from the parent's URL).
pub(crate) fn fetch_into(repo_path: &Path, url: &str) -> Result<(), Error> {
    use gix::remote::Direction;

    let repo = gix::open(repo_path)?;
    let parsed_url = gix::url::Url::try_from(url)
        .map_err(|e| Error::Other(format!("invalid url {url:?}: {e}")))?;
    let remote = repo
        .remote_at(parsed_url)
        .map_err(|e| Error::Git(Box::new(e)))?
        .with_refspecs(
            [b"+refs/heads/main:refs/heads/main".as_bstr()],
            Direction::Fetch,
        )
        .map_err(|e| Error::Git(Box::new(e)))?;
    do_fetch(remote)
}

fn validate_remote_name(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::Other("remote name is empty".into()));
    }
    if name
        .chars()
        .any(|c| c == '"' || c == '\n' || c == '\r' || c == '[' || c == ']')
    {
        return Err(Error::Other(format!(
            "remote name {name:?} contains invalid characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(Remotes::new(&g).list().unwrap().is_empty());
    }

    #[test]
    fn add_then_list_roundtrips() {
        let (_d, g) = fixture();
        let r = Remotes::new(&g);
        r.add("origin", "https://example.com/repo.git").unwrap();
        let remotes = r.list().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "https://example.com/repo.git");
    }

    #[test]
    fn add_duplicate_name_errors() {
        let (_d, g) = fixture();
        let r = Remotes::new(&g);
        r.add("origin", "url1").unwrap();
        assert!(r.add("origin", "url2").is_err());
    }

    #[test]
    fn remove_removes_entry() {
        let (_d, g) = fixture();
        let r = Remotes::new(&g);
        r.add("origin", "url").unwrap();
        r.remove("origin").unwrap();
        assert!(r.list().unwrap().is_empty());
    }

    #[test]
    fn remove_unknown_errors() {
        let (_d, g) = fixture();
        assert!(Remotes::new(&g).remove("origin").is_err());
    }

    #[test]
    fn preserves_unrelated_sections() {
        let (_d, g) = fixture();
        let r = Remotes::new(&g);
        r.add("origin", "url").unwrap();
        r.remove("origin").unwrap();
        let text = fs::read_to_string(GitConfig::path(&g)).unwrap();
        assert!(text.contains("[core]"));
        assert!(text.contains("bare = true"));
    }

    #[test]
    fn multiple_remotes_coexist() {
        let (_d, g) = fixture();
        let r = Remotes::new(&g);
        r.add("a", "url-a").unwrap();
        r.add("b", "url-b").unwrap();
        let remotes = r.list().unwrap();
        assert_eq!(remotes.len(), 2);
        let names: Vec<_> = remotes.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn rejects_name_with_quote() {
        let (_d, g) = fixture();
        assert!(Remotes::new(&g).add("bad\"name", "url").is_err());
    }

    #[test]
    fn list_missing_config_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Remotes::new(dir.path()).list().unwrap().is_empty());
    }
}
