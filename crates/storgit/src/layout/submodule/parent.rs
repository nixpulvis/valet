use std::collections::BTreeMap;
use std::path::Path;

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::{Tree, tree::EntryKind};

use crate::error::Error;
use crate::git::decode_tree;
use crate::id::EntryId;

/// Subtree name inside the parent's root tree under which storgit
/// caches each live module's current label blob, keyed by module id.
/// Modules with empty label have no entry here.
pub(crate) const INDEX_DIR: &str = "index";
/// Filename of the standard git submodule manifest in the parent's
/// root tree. storgit writes this so plain `git` tooling on an
/// extracted tarball recognises the gitlink entries as submodules.
/// Sorts before any valid [`EntryId`] (leading `.` is forbidden) so it
/// always appears first in the parent tree.
pub(crate) const GITMODULES_FILE: &str = ".gitmodules";

/// The two derived maps a submodule-layout parent repo carries: the
/// gitlink set (one pointer per live id) and the label cache (one
/// blob per live id with a non-empty label). Paired so callers can
/// load them in one pass and so the `.gitmodules` manifest has a
/// natural receiver.
#[derive(Debug, Default, Clone)]
pub(crate) struct ParentTree {
    pub(crate) gitlinks: BTreeMap<EntryId, gix::ObjectId>,
    pub(crate) labels: BTreeMap<EntryId, Vec<u8>>,
}

impl ParentTree {
    /// Seed from the persisted parent repo. The gitlink map comes
    /// from the root tree's commit-mode entries; the label cache
    /// comes from the `index/` subtree's blobs (if present). Returns
    /// an empty tree when the parent has no HEAD yet (fresh repo).
    ///
    /// Filenames read back here were written by storgit from
    /// validated [`EntryId`]s, so [`EntryId::new`] should succeed.
    /// Any failure here means on-disk corruption; surface it as
    /// [`Error::Other`].
    pub(crate) fn load(parent_path: &Path) -> Result<Self, Error> {
        let parent = gix::open(parent_path)?;
        let Some(tree) = current_parent_tree(&parent)? else {
            return Ok(Self::default());
        };
        let mut gitlinks = BTreeMap::new();
        let mut index_tree_id: Option<gix::ObjectId> = None;
        for entry in tree.entries {
            match entry.mode.kind() {
                EntryKind::Commit => {
                    let id = entry_filename_as_id(&entry.filename)?;
                    gitlinks.insert(id, entry.oid);
                }
                EntryKind::Tree if entry.filename.as_bstr() == BStr::new(INDEX_DIR) => {
                    index_tree_id = Some(entry.oid);
                }
                _ => {}
            }
        }

        let mut labels = BTreeMap::new();
        if let Some(index_tree_id) = index_tree_id {
            let index_tree = decode_tree(&parent, index_tree_id)?;
            for entry in index_tree.entries {
                if matches!(entry.mode.kind(), EntryKind::Blob) {
                    let blob = parent.find_object(entry.oid)?;
                    let id = entry_filename_as_id(&entry.filename)?;
                    labels.insert(id, blob.data.clone());
                }
            }
        }
        Ok(Self { gitlinks, labels })
    }

    /// Read the gitlink set at the parent commit `commit`, ignoring
    /// non-gitlink tree entries (`.gitmodules`, `index/`). Used by
    /// the merge kernel to discover the incoming and base gitlink
    /// sets when merging two parent histories.
    pub(crate) fn gitlinks_at(
        parent_path: &Path,
        commit: gix::ObjectId,
    ) -> Result<BTreeMap<EntryId, gix::ObjectId>, Error> {
        let repo = gix::open(parent_path)?;
        let tree_id = repo.find_object(commit)?.into_commit().decode()?.tree();
        let tree = decode_tree(&repo, tree_id)?;
        let mut out = BTreeMap::new();
        for entry in tree.entries {
            if !matches!(entry.mode.kind(), EntryKind::Commit) {
                continue;
            }
            let id = entry_filename_as_id(&entry.filename)?;
            out.insert(id, entry.oid);
        }
        Ok(out)
    }
}

/// Serialise a gitlink map as a `.gitmodules` config file. One
/// stanza per id; `path` is the gitlink filename in the parent tree
/// (the bare id), `url` is a path relative to the parent repo
/// pointing at the module's bare repo on disk. The relative URL
/// keeps an extracted tarball self-contained: a
/// `git clone --recursive` against the parent finds each submodule
/// next door.
///
/// [`EntryId`] forbids the only characters that would need escaping
/// in a git-config quoted section name (`"` and `\`), so we can
/// interpolate directly without escaping.
pub(crate) fn serialize_gitmodules(gitlinks: &BTreeMap<EntryId, gix::ObjectId>) -> Vec<u8> {
    let mut out = String::new();
    for id in gitlinks.keys() {
        let s = id.as_str();
        out.push_str("[submodule \"");
        out.push_str(s);
        out.push_str("\"]\n\tpath = ");
        out.push_str(s);
        out.push_str("\n\turl = ../modules/");
        out.push_str(s);
        out.push_str(".git\n");
    }
    out.into_bytes()
}

/// Turn a git tree entry's filename back into a validated
/// [`EntryId`]. Only returns an error when the on-disk state is
/// corrupt (a filename that storgit would never have written).
fn entry_filename_as_id(filename: &BString) -> Result<EntryId, Error> {
    let s = filename.to_string();
    EntryId::new(s)
        .map_err(|e| Error::Other(format!("corrupt parent tree entry {filename:?}: {e}")))
}

/// Return the parent repo's current root tree, or `None` if HEAD is
/// unborn (i.e. the parent has never been committed to).
fn current_parent_tree(repo: &gix::Repository) -> Result<Option<Tree>, Error> {
    let Ok(head) = repo.head_commit() else {
        return Ok(None);
    };
    let tree = head.tree()?;
    Ok(Some(decode_tree(repo, tree.id().detach())?))
}
