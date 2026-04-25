//! [`Remote`] and the fetch primitives that drive a `gix::Remote` to
//! actually move objects.
//!
//! Remotes live in the layout's [git_dir](crate::Layout::git_dir) under
//! standard `[remote "<name>"]` sections (parsed by
//! [`crate::config::GitConfig`]), so `gix::remote` and every other git
//! tool see them transparently.

use std::path::Path;

use gix::bstr::ByteSlice;

use crate::error::Error;

/// A named remote: a name and a URL. Pure data; the I/O lives on
/// whatever holds the git directory the remote is configured in
/// (today: [`crate::Store::fetch`]).
///
/// Constructed by reading the bare repo's git config; callers
/// reach `Remote` instances through [`crate::Store::remotes`] and
/// the per-name fetch path on [`crate::Store::fetch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub name: String,
    pub url: String,
}

/// Drive a configured `gix::Remote` through connect -> prepare ->
/// receive with storgit's canonical no-progress, non-interruptible
/// settings. Shared by [`fetch_into`] (ad-hoc URL) and
/// [`Remote::fetch`] (configured remote by name).
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
/// (e.g. a per-submodule URL derived from the parent's URL) without
/// registering the URL as a configured remote.
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
