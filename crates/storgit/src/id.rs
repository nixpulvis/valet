//! Identifier types used by storgit.

use std::borrow::Borrow;
use std::str::FromStr;

/// A validated entry identifier.
///
/// Constructed via [`EntryId::new`] (or `s.parse::<EntryId>()`); the
/// validating constructor enforces every constraint storgit needs to
/// safely use the id as a filename (`modules/<id>.git`), as a git tree
/// entry name, and as a key in the parent tree's gitlink set.
///
/// Rules enforced:
/// - non-empty
/// - at most [`EntryId::MAX_LEN`] bytes
/// - no `/` (would create a subdirectory in `modules/`)
/// - no `"` or `\` (would need escaping inside the `.gitmodules`
///   section name storgit writes for plain-`git` interop)
/// - no ASCII control characters (`< 0x20` or DEL, including `\0`,
///   `\n`, `\t`); they would corrupt git tree filenames or the
///   `.gitmodules` config file
/// - no leading `.` (rejects `.`, `..`, hidden-file ids)
/// - does not end in `.git` (collides with the `<id>.git` module dir)
/// - is not a [reserved name](EntryId::is_reserved) used by storgit
///   internally (currently just the string `"index"`, which collides
///   with the parent tree's index subtree)
///
/// Holding an [`EntryId`] value is therefore proof that the bytes are
/// safe to plug into all of storgit's internal paths and tree writes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(String);

/// Reasons [`EntryId::new`] can reject a candidate id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryIdError {
    Empty,
    TooLong { len: usize, max: usize },
    BadChar(char),
    LeadingDot,
    GitSuffix,
    Reserved,
}

impl EntryId {
    /// Maximum byte length of a valid id. Picked to leave headroom
    /// under typical filesystem name limits (255 bytes on most
    /// filesystems) once the `<id>.git` suffix is appended.
    pub const MAX_LEN: usize = 240;

    /// True when `s` is a reserved name storgit uses for its own
    /// bookkeeping inside the parent tree, and therefore must not be
    /// used as an entry id.
    pub fn is_reserved(s: &str) -> bool {
        s == crate::layout::submodule::parent::INDEX_DIR
    }

    /// Construct a validated id, returning [`EntryIdError`] on rejection.
    pub fn new(s: impl Into<String>) -> Result<Self, EntryIdError> {
        let s = s.into();
        Self::validate(&s)?;
        Ok(EntryId(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(s: &str) -> Result<(), EntryIdError> {
        if s.is_empty() {
            return Err(EntryIdError::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(EntryIdError::TooLong {
                len: s.len(),
                max: Self::MAX_LEN,
            });
        }
        if s.starts_with('.') {
            return Err(EntryIdError::LeadingDot);
        }
        if s.ends_with(".git") {
            return Err(EntryIdError::GitSuffix);
        }
        if Self::is_reserved(s) {
            return Err(EntryIdError::Reserved);
        }
        for c in s.chars() {
            if c == '/' || c == '"' || c == '\\' {
                return Err(EntryIdError::BadChar(c));
            }
            let code = c as u32;
            if code < 0x20 || code == 0x7f {
                return Err(EntryIdError::BadChar(c));
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for EntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for EntryIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntryIdError::Empty => write!(f, "id is empty"),
            EntryIdError::TooLong { len, max } => write!(f, "id is {len} bytes; max is {max}"),
            EntryIdError::BadChar(c) => write!(f, "id contains forbidden character {c:?}"),
            EntryIdError::LeadingDot => write!(f, "id may not start with '.'"),
            EntryIdError::GitSuffix => write!(f, "id may not end with '.git'"),
            EntryIdError::Reserved => write!(f, "id is reserved by storgit"),
        }
    }
}

impl std::error::Error for EntryIdError {}

impl AsRef<str> for EntryId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for EntryId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for EntryId {
    type Err = EntryIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        EntryId::new(s.to_string())
    }
}

/// A git commit identifier (SHA-1, 20 bytes).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitId([u8; 20]);

impl CommitId {
    /// Number of hex characters used by [`CommitId::to_short_hex`],
    /// matching git's default abbreviated SHA length.
    pub const SHORT_HEX_LEN: usize = 7;

    /// Raw SHA-1 bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Full 40-character lowercase hex SHA-1, as git prints with `%H`.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(40);
        for b in &self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Abbreviated hex SHA-1 (first [`CommitId::SHORT_HEX_LEN`]
    /// characters), as git prints with `%h`.
    pub fn to_short_hex(&self) -> String {
        let mut s = self.to_hex();
        s.truncate(Self::SHORT_HEX_LEN);
        s
    }
}

impl From<gix::ObjectId> for CommitId {
    fn from(id: gix::ObjectId) -> Self {
        let slice = id.as_slice();
        let mut out = [0u8; 20];
        out.copy_from_slice(&slice[..20]);
        CommitId(out)
    }
}
