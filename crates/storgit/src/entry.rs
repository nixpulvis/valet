use std::time::SystemTime;

/// A git commit identifier (SHA-1, 20 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId(pub [u8; 20]);

impl From<gix::ObjectId> for CommitId {
    fn from(id: gix::ObjectId) -> Self {
        let slice = id.as_slice();
        let mut out = [0u8; 20];
        out.copy_from_slice(&slice[..20]);
        CommitId(out)
    }
}

/// A single historical version of an entry. Every live commit carries
/// a `(label, data)` pair inside the module's tree; a tombstone commit
/// written by [`crate::Store::archive`] has an empty tree and surfaces
/// as `label = None, data = None`.
#[derive(Debug, Clone)]
pub struct Entry {
    pub commit: CommitId,
    pub time: SystemTime,
    pub label: Option<Vec<u8>>,
    pub data: Option<Vec<u8>>,
}
