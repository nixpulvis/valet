use std::time::{Duration, UNIX_EPOCH};

use gix::bstr::{BStr, BString, ByteSlice};
use gix::objs::{
    Tree,
    tree::{Entry as TreeEntry, EntryKind},
};

use crate::entry::Entry;
use crate::error::Error;
use crate::git::{decode_tree, module_ref_path, read_ref_file, write_commit, write_ref_file};

/// Filenames used within a submodule commit's tree for the payload
/// and the searchable label. Each live commit carries one or both;
/// a tombstone has neither (empty tree).
pub(crate) const DATA_FILE: &str = "data";
pub(crate) const LABEL_FILE: &str = "label";

/// Write a tombstone commit (empty tree) for a submodule, chaining it
/// onto whatever commit the module's branch currently points at. New
/// objects land in the module's own object DB; the module's
/// `refs/heads/main` file is updated in place.
pub(crate) fn write_tombstone_commit(module: &gix::Repository) -> Result<gix::ObjectId, Error> {
    let module_path = module.path();
    let tree = Tree {
        entries: Vec::new(),
    };
    let tree_id = module.write_object(&tree)?.detach();
    let prior_commit = read_ref_file(&module_ref_path(module_path))?;
    let commit_id = write_commit(
        module,
        tree_id,
        prior_commit.into_iter().collect(),
        "archive",
    )?;
    write_ref_file(&module_ref_path(module_path), commit_id)?;
    Ok(commit_id)
}

/// Write a commit for a submodule whose tree contains up to one blob
/// per slot. For each of [`DATA_FILE`] and [`LABEL_FILE`]: `Some(bytes)`
/// writes a new blob; `None` carries the prior HEAD tree's blob forward
/// (or omits the slot entirely when there is no prior commit). New
/// objects go to the module's own object DB; the module's
/// `refs/heads/main` is updated.
///
/// Returns `Ok(None)` when the newly-built tree is byte-identical to
/// the tree at the module's current HEAD. Git's content-addressing
/// guarantees identical tree bytes produce identical oids, so a single
/// oid equality check against the prior commit's tree is sufficient.
pub(crate) fn write_entry_commit(
    module: &gix::Repository,
    label: Option<&[u8]>,
    data: Option<&[u8]>,
) -> Result<Option<gix::ObjectId>, Error> {
    let module_path = module.path();
    let prior_commit = read_ref_file(&module_ref_path(module_path))?;
    // Only decode the prior tree when we actually need a blob from it.
    let prior_tree_entries = if (label.is_none() || data.is_none())
        && let Some(prior_id) = prior_commit
    {
        let prior = module.find_object(prior_id)?.into_commit();
        let prior_tree_id = prior.decode()?.tree();
        Some(decode_tree(module, prior_tree_id)?.entries)
    } else {
        None
    };
    let prior_blob = |filename: &str| -> Option<gix::ObjectId> {
        prior_tree_entries.as_ref().and_then(|entries| {
            entries
                .iter()
                .find(|e| e.filename.as_bstr() == BStr::new(filename))
                .map(|e| e.oid)
        })
    };

    let mut entries: Vec<TreeEntry> = Vec::with_capacity(2);
    let data_oid = match data {
        Some(bytes) => Some(module.write_blob(bytes)?.detach()),
        None => prior_blob(DATA_FILE),
    };
    if let Some(oid) = data_oid {
        entries.push(TreeEntry {
            mode: EntryKind::Blob.into(),
            filename: BString::from(DATA_FILE),
            oid,
        });
    }
    let label_oid = match label {
        Some(bytes) => Some(module.write_blob(bytes)?.detach()),
        None => prior_blob(LABEL_FILE),
    };
    if let Some(oid) = label_oid {
        entries.push(TreeEntry {
            mode: EntryKind::Blob.into(),
            filename: BString::from(LABEL_FILE),
            oid,
        });
    }
    // DATA_FILE < LABEL_FILE lexicographically, so push order already
    // satisfies git's strict filename sort.
    let tree = Tree { entries };
    let tree_id = module.write_object(&tree)?.detach();

    // No-op detection: if the module already has a HEAD whose tree
    // oid matches the tree we just built, every file (name, mode,
    // contents) is unchanged and we skip writing an identical commit.
    if let Some(prior_id) = prior_commit {
        let prior = module.find_object(prior_id)?.into_commit();
        let prior_tree_id = prior.decode()?.tree();
        if prior_tree_id == tree_id {
            return Ok(None);
        }
    }

    let commit_id = write_commit(module, tree_id, prior_commit.into_iter().collect(), "put")?;
    write_ref_file(&module_ref_path(module_path), commit_id)?;
    Ok(Some(commit_id))
}

/// Build an [`Entry`] for the commit at `commit_id` in `repo`. Reads
/// the commit's time and its tree's `label` / `data` blobs; each slot
/// is `Some(bytes)` if the corresponding file exists in the tree,
/// `None` if absent. Tombstone commits (empty tree) surface as both
/// slots `None`.
pub(crate) fn read_entry_at(
    repo: &gix::Repository,
    commit_id: gix::ObjectId,
) -> Result<Entry, Error> {
    let commit = repo.find_object(commit_id)?.into_commit();
    let sig = commit.committer()?;
    let seconds = sig.seconds().max(0) as u64;
    let time = UNIX_EPOCH + Duration::from_secs(seconds);
    let tree_id = commit.decode()?.tree();
    let tree = decode_tree(repo, tree_id)?;
    let mut label = None;
    let mut data = None;
    for entry in tree.entries {
        if entry.filename.as_bstr() == BStr::new(DATA_FILE) {
            data = Some(repo.find_object(entry.oid)?.data.clone());
        } else if entry.filename.as_bstr() == BStr::new(LABEL_FILE) {
            label = Some(repo.find_object(entry.oid)?.data.clone());
        }
    }
    Ok(Entry {
        commit: commit_id.into(),
        time,
        label,
        data,
    })
}
