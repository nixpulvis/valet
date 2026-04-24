//! The subdir layout: a single bare repo whose tree carries all
//! entries as `records/<id>/{data,label}`. One shared ref advances on
//! every write; per-entry history comes from path-scoped walks of the
//! shared commit graph rather than isolated submodule refs.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::{
    Tree,
    tree::{Entry as TreeEntry, EntryKind},
};

use crate::entry::{CommitId, Entry};
use crate::error::Error;
use crate::git::{
    BRANCH, decode_tree, init_bare_on_branch, module_ref_path, read_ref_file, write_commit,
    write_ref_file,
};
use crate::id::Id;
use crate::layout::Layout;
use crate::module::{DATA_FILE, LABEL_FILE};
use crate::tarball::tar_dir;

/// Subtree name inside the repo's root tree that holds every entry.
const RECORDS_DIR: &str = "records";

/// The subdir-layout storgit implementation. Backed by a single bare
/// repo at `path`. Records live at `records/<id>/{data,label}` in
/// that repo's tree; a single shared ref (`refs/heads/main`) advances
/// with every write.
pub struct SubdirLayout {
    path: PathBuf,
    label_cache: BTreeMap<Id, Vec<u8>>,
}

impl SubdirLayout {
    fn open_repo(&self) -> Result<gix::Repository, Error> {
        Ok(gix::open(&self.path)?)
    }

    fn head_commit(&self, _repo: &gix::Repository) -> Result<Option<gix::ObjectId>, Error> {
        read_ref_file(&module_ref_path(&self.path))
    }

    /// Tree oid of `records/<id>/` at the given root tree, if any.
    fn id_subtree_oid(
        repo: &gix::Repository,
        root_tree_id: gix::ObjectId,
        id: &Id,
    ) -> Result<Option<gix::ObjectId>, Error> {
        let Some(records_oid) = subtree_entry(repo, root_tree_id, RECORDS_DIR)? else {
            return Ok(None);
        };
        subtree_entry(repo, records_oid, id.as_str())
    }

    /// Build and persist the label_cache by walking `HEAD:records/`.
    fn rebuild_label_cache(&mut self) -> Result<(), Error> {
        self.label_cache.clear();
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(());
        };
        let commit = repo.find_object(head)?.into_commit();
        let root_tree_id = commit.decode()?.tree();
        let Some(records_oid) = subtree_entry(&repo, root_tree_id, RECORDS_DIR)? else {
            return Ok(());
        };
        let records_tree = decode_tree(&repo, records_oid)?;
        for entry in records_tree.entries {
            if !matches!(entry.mode.kind(), EntryKind::Tree) {
                continue;
            }
            let id = Id::new(entry.filename.to_string()).map_err(|e| {
                Error::Other(format!(
                    "corrupt records/ entry {:?}: {e}",
                    entry.filename
                ))
            })?;
            let id_tree = decode_tree(&repo, entry.oid)?;
            if let Some(label_entry) = id_tree
                .entries
                .iter()
                .find(|e| e.filename.as_bstr() == BStr::new(LABEL_FILE))
            {
                let blob = repo.find_object(label_entry.oid)?;
                if !blob.data.is_empty() {
                    self.label_cache.insert(id, blob.data.clone());
                }
            }
        }
        Ok(())
    }
}

/// Look up a named child of `tree_id` and return its oid, or `None`
/// if that child doesn't exist. Matches by byte-wise filename.
fn subtree_entry(
    repo: &gix::Repository,
    tree_id: gix::ObjectId,
    name: &str,
) -> Result<Option<gix::ObjectId>, Error> {
    let tree = decode_tree(repo, tree_id)?;
    Ok(tree
        .entries
        .into_iter()
        .find(|e| e.filename.as_bstr() == BStr::new(name))
        .map(|e| e.oid))
}

impl Layout for SubdirLayout {
    fn new(path: PathBuf) -> Result<Self, Error> {
        if path.exists() {
            return Err(Error::Other(format!(
                "subdir new: path {path:?} already exists"
            )));
        }
        std::fs::create_dir(&path)?;
        init_bare_on_branch(&path)?;
        Ok(SubdirLayout {
            path,
            label_cache: BTreeMap::new(),
        })
    }

    fn open(path: PathBuf) -> Result<Self, Error> {
        validate_subdir_repo(&path)?;
        let mut layout = SubdirLayout {
            path,
            label_cache: BTreeMap::new(),
        };
        layout.rebuild_label_cache()?;
        Ok(layout)
    }

    fn save(&mut self) -> Result<Vec<u8>, Error> {
        tar_dir(&self.path)
    }

    fn put(
        &mut self,
        id: &Id,
        label: Option<&[u8]>,
        data: Option<&[u8]>,
    ) -> Result<Option<CommitId>, Error> {
        if label.is_none() && data.is_none() {
            return Err(Error::Other(
                "Store::put requires at least one of label or data; use Store::archive for a tombstone"
                    .into(),
            ));
        }
        let repo = self.open_repo()?;
        let prior_commit = self.head_commit(&repo)?;
        let prior_root_tree_id = if let Some(pid) = prior_commit {
            Some(repo.find_object(pid)?.into_commit().decode()?.tree())
        } else {
            None
        };
        let prior_records_oid = match prior_root_tree_id {
            Some(tid) => subtree_entry(&repo, tid, RECORDS_DIR)?,
            None => None,
        };
        let prior_id_subtree_oid = match prior_records_oid {
            Some(roid) => subtree_entry(&repo, roid, id.as_str())?,
            None => None,
        };
        let prior_id_entries = match prior_id_subtree_oid {
            Some(sid) => Some(decode_tree(&repo, sid)?.entries),
            None => None,
        };
        let prior_blob = |filename: &str| -> Option<gix::ObjectId> {
            prior_id_entries.as_ref().and_then(|entries| {
                entries
                    .iter()
                    .find(|e| e.filename.as_bstr() == BStr::new(filename))
                    .map(|e| e.oid)
            })
        };

        let data_oid = match data {
            Some(bytes) => Some(repo.write_blob(bytes)?.detach()),
            None => prior_blob(DATA_FILE),
        };
        let label_oid = match label {
            Some(bytes) => Some(repo.write_blob(bytes)?.detach()),
            None => prior_blob(LABEL_FILE),
        };

        let mut id_entries: Vec<TreeEntry> = Vec::with_capacity(2);
        if let Some(oid) = data_oid {
            id_entries.push(TreeEntry {
                mode: EntryKind::Blob.into(),
                filename: BString::from(DATA_FILE),
                oid,
            });
        }
        if let Some(oid) = label_oid {
            id_entries.push(TreeEntry {
                mode: EntryKind::Blob.into(),
                filename: BString::from(LABEL_FILE),
                oid,
            });
        }
        // DATA_FILE < LABEL_FILE lexicographically, so push order already
        // satisfies git's strict filename sort.
        let id_tree = Tree {
            entries: id_entries,
        };
        let id_tree_id = repo.write_object(&id_tree)?.detach();

        // No-op detection: identical id-subtree means identical commit
        // for this record.
        if prior_id_subtree_oid == Some(id_tree_id) {
            return Ok(None);
        }

        let records_tree_id = write_records_tree(&repo, prior_records_oid, id, Some(id_tree_id))?;
        let root_tree_id = write_root_tree(&repo, prior_root_tree_id, records_tree_id)?;
        let commit_id = write_commit(
            &repo,
            root_tree_id,
            prior_commit.into_iter().collect(),
            "put",
        )?;
        write_ref_file(&module_ref_path(&self.path), commit_id)?;

        match label {
            Some(bytes) if !bytes.is_empty() => {
                self.label_cache.insert(id.clone(), bytes.to_vec());
            }
            Some(_) => {
                self.label_cache.remove(id);
            }
            None => {}
        }
        Ok(Some(commit_id.into()))
    }

    fn get(&self, id: &Id) -> Result<Option<Entry>, Error> {
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(None);
        };
        let commit = repo.find_object(head)?.into_commit();
        let root_tree_id = commit.decode()?.tree();
        if Self::id_subtree_oid(&repo, root_tree_id, id)?.is_none() {
            return Ok(None);
        }
        Ok(Some(read_subdir_entry_at(&repo, head, id)?))
    }

    fn archive(&mut self, id: &Id) -> Result<(), Error> {
        let repo = self.open_repo()?;
        let Some(prior_commit) = self.head_commit(&repo)? else {
            return Ok(());
        };
        let prior_root_tree_id = repo.find_object(prior_commit)?.into_commit().decode()?.tree();
        let prior_records_oid = subtree_entry(&repo, prior_root_tree_id, RECORDS_DIR)?;
        let Some(roid) = prior_records_oid else {
            return Ok(());
        };
        if subtree_entry(&repo, roid, id.as_str())?.is_none() {
            return Ok(());
        }
        let records_tree_id = write_records_tree(&repo, Some(roid), id, None)?;
        let root_tree_id = write_root_tree(&repo, Some(prior_root_tree_id), records_tree_id)?;
        let commit_id = write_commit(&repo, root_tree_id, vec![prior_commit], "archive")?;
        write_ref_file(&module_ref_path(&self.path), commit_id)?;
        self.label_cache.remove(id);
        Ok(())
    }

    fn delete(&mut self, id: &Id) -> Result<(), Error> {
        // A single-ref layout can't cheaply erase history for one
        // record without rewriting the ref. For now `delete` behaves
        // like `archive`: the entry is removed from the tree, and the
        // past commits that touched it remain reachable from HEAD.
        self.archive(id)
    }

    fn list(&self) -> Result<Vec<Id>, Error> {
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(Vec::new());
        };
        let commit = repo.find_object(head)?.into_commit();
        let root_tree_id = commit.decode()?.tree();
        let Some(records_oid) = subtree_entry(&repo, root_tree_id, RECORDS_DIR)? else {
            return Ok(Vec::new());
        };
        let records_tree = decode_tree(&repo, records_oid)?;
        let mut out = Vec::with_capacity(records_tree.entries.len());
        for entry in records_tree.entries {
            if !matches!(entry.mode.kind(), EntryKind::Tree) {
                continue;
            }
            let id = Id::new(entry.filename.to_string()).map_err(|e| {
                Error::Other(format!(
                    "corrupt records/ entry {:?}: {e}",
                    entry.filename
                ))
            })?;
            out.push(id);
        }
        Ok(out)
    }

    fn history(&self, id: &Id) -> Result<Vec<Entry>, Error> {
        let repo = self.open_repo()?;
        let Some(head) = self.head_commit(&repo)? else {
            return Ok(Vec::new());
        };
        let head_obj = repo.find_object(head)?.into_commit();
        let mut out = Vec::new();
        for info in head_obj.ancestors().all()? {
            let info = info?;
            let commit = repo.find_object(info.id)?.into_commit();
            let decoded = commit.decode()?;
            let tree_id = decoded.tree();
            let this_subtree = Self::id_subtree_oid(&repo, tree_id, id)?;
            let parent_ids: Vec<gix::ObjectId> = decoded.parents().collect();
            let parent_subtree = if let Some(&pid) = parent_ids.first() {
                let pc = repo.find_object(pid)?.into_commit();
                let ptid = pc.decode()?.tree();
                Self::id_subtree_oid(&repo, ptid, id)?
            } else {
                None
            };
            if this_subtree != parent_subtree {
                out.push(read_subdir_entry_at(&repo, info.id, id)?);
            }
        }
        Ok(out)
    }

    fn label(&self, id: &Id) -> Option<&[u8]> {
        self.label_cache.get(id).map(Vec::as_slice)
    }

    fn list_labels(&self) -> Vec<(Id, Vec<u8>)> {
        self.label_cache
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Build a new `records/` subtree by copying the entries of the prior
/// records subtree (if any), removing or replacing the entry for
/// `id`, and writing the result. `new_id_subtree` of `None` removes
/// the id; `Some(oid)` replaces or inserts it.
fn write_records_tree(
    repo: &gix::Repository,
    prior_records_oid: Option<gix::ObjectId>,
    id: &Id,
    new_id_subtree: Option<gix::ObjectId>,
) -> Result<Option<gix::ObjectId>, Error> {
    let mut entries: Vec<TreeEntry> = match prior_records_oid {
        Some(roid) => decode_tree(repo, roid)?
            .entries
            .into_iter()
            .filter(|e| e.filename.as_bstr() != BStr::new(id.as_str()))
            .collect(),
        None => Vec::new(),
    };
    if let Some(oid) = new_id_subtree {
        entries.push(TreeEntry {
            mode: EntryKind::Tree.into(),
            filename: BString::from(id.as_str()),
            oid,
        });
    }
    if entries.is_empty() {
        return Ok(None);
    }
    entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    let tree = Tree { entries };
    Ok(Some(repo.write_object(&tree)?.detach()))
}

/// Build the new root tree by copying prior root entries (if any),
/// removing or replacing the `records/` entry, and writing the
/// result. `new_records` of `None` drops the `records/` subtree.
fn write_root_tree(
    repo: &gix::Repository,
    prior_root_tree_id: Option<gix::ObjectId>,
    new_records: Option<gix::ObjectId>,
) -> Result<gix::ObjectId, Error> {
    let mut entries: Vec<TreeEntry> = match prior_root_tree_id {
        Some(tid) => decode_tree(repo, tid)?
            .entries
            .into_iter()
            .filter(|e| e.filename.as_bstr() != BStr::new(RECORDS_DIR))
            .collect(),
        None => Vec::new(),
    };
    if let Some(oid) = new_records {
        entries.push(TreeEntry {
            mode: EntryKind::Tree.into(),
            filename: BString::from(RECORDS_DIR),
            oid,
        });
    }
    entries.sort_by(|a, b| a.filename.cmp(&b.filename));
    let tree = Tree { entries };
    Ok(repo.write_object(&tree)?.detach())
}

/// Read a single [`Entry`] for `id` at commit `commit_id`. If the
/// `records/<id>/` subtree is absent at that commit (the archive
/// point), returns an entry with both slots `None`.
fn read_subdir_entry_at(
    repo: &gix::Repository,
    commit_id: gix::ObjectId,
    id: &Id,
) -> Result<Entry, Error> {
    let commit = repo.find_object(commit_id)?.into_commit();
    let sig = commit.committer()?;
    let seconds = sig.seconds().max(0) as u64;
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
    let root_tree_id = commit.decode()?.tree();
    let id_subtree_oid = SubdirLayout::id_subtree_oid(repo, root_tree_id, id)?;
    let (label, data) = match id_subtree_oid {
        Some(sid) => {
            let id_tree = decode_tree(repo, sid)?;
            let mut label = None;
            let mut data = None;
            for entry in id_tree.entries {
                if entry.filename.as_bstr() == BStr::new(DATA_FILE) {
                    data = Some(repo.find_object(entry.oid)?.data.clone());
                } else if entry.filename.as_bstr() == BStr::new(LABEL_FILE) {
                    label = Some(repo.find_object(entry.oid)?.data.clone());
                }
            }
            (label, data)
        }
        None => (None, None),
    };
    Ok(Entry {
        commit: commit_id.into(),
        time,
        label,
        data,
    })
}

/// Sanity-check that `path` holds a valid subdir-layout storgit
/// repo: a bare repo whose HEAD points to storgit's branch and
/// whose root tree (if any commit exists) contains no gitlinks
/// (which would signal a submodule-layout repo).
fn validate_subdir_repo(path: &Path) -> Result<(), Error> {
    let repo = gix::open(path).map_err(|e| {
        Error::Other(format!(
            "storgit subdir open: path {path:?} is not a git repo: {e}"
        ))
    })?;
    let head_raw = std::fs::read_to_string(path.join("HEAD"))
        .map_err(|e| Error::Other(format!("storgit subdir open: cannot read HEAD: {e}")))?;
    let head_trimmed = head_raw.trim();
    let expected = format!("ref: {BRANCH}");
    if head_trimmed != expected {
        return Err(Error::Other(format!(
            "storgit subdir open: HEAD must be {expected:?}; got {head_trimmed:?}"
        )));
    }
    if let Some(commit_oid) = read_ref_file(&module_ref_path(path))? {
        let commit = repo.find_object(commit_oid)?.into_commit();
        let tree_id = commit.decode()?.tree();
        let tree = decode_tree(&repo, tree_id)?;
        for entry in tree.entries {
            if matches!(entry.mode.kind(), EntryKind::Commit) {
                return Err(Error::Other(format!(
                    "storgit subdir open: {path:?} looks like a submodule-layout storgit repo (gitlinks in root tree)"
                )));
            }
        }
    }
    Ok(())
}

// Subdir layout's new/open/save/load all live on the Layout trait;
// no Store<SubdirLayout> inherent methods are needed.
