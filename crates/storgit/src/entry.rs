use std::time::SystemTime;

use crate::id::CommitId;

/// A single historical version of an entry.
///
/// Every live commit carries a `(label, data)` pair inside the module's tree; a
/// tombstone commit written by [`archive`] has an empty tree and surfaces as
/// `label = None, data = None`.
///
/// [`archive`]: crate::Store::archive
#[derive(Debug, Clone)]
pub struct Entry {
    /// Commit that produced this version of the entry.
    pub commit: CommitId,
    /// Committer timestamp of the commit.
    pub time: SystemTime,
    /// Caller-defined metadata the caller wants to scan cheaply
    /// without reading the full record. `None` on a tombstone or
    /// when no label has ever been written for this entry.
    pub label: Option<Vec<u8>>,
    /// The entry's payload bytes. `None` on a tombstone or when only
    /// a label has been written for this entry.
    pub data: Option<Vec<u8>>,
}
