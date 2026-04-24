use std::borrow::Borrow;
use std::str::FromStr;

/// A validated entry identifier. Constructed via [`Id::new`] (or
/// `s.parse::<Id>()`); the validating constructor enforces every
/// constraint storgit needs to safely use the id as a filename
/// (`modules/<id>.git`), as a git tree entry name, and as a key in
/// the parent tree's gitlink set.
///
/// Rules enforced:
/// - non-empty
/// - at most [`Id::MAX_LEN`] bytes
/// - no `/` (would create a subdirectory in `modules/`)
/// - no `"` or `\` (would need escaping inside the `.gitmodules`
///   section name storgit writes for plain-`git` interop)
/// - no ASCII control characters (`< 0x20` or DEL, including `\0`,
///   `\n`, `\t`); they would corrupt git tree filenames or the
///   `.gitmodules` config file
/// - no leading `.` (rejects `.`, `..`, hidden-file ids)
/// - does not end in `.git` (collides with the `<id>.git` module dir)
/// - is not a [reserved name](Id::is_reserved) used by storgit
///   internally (currently just the string `"index"`, which collides
///   with the parent tree's index subtree)
///
/// Holding an [`Id`] value is therefore proof that the bytes are safe
/// to plug into all of storgit's internal paths and tree writes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(String);

/// Reasons [`Id::new`] can reject a candidate id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Empty,
    TooLong { len: usize, max: usize },
    BadChar(char),
    LeadingDot,
    GitSuffix,
    Reserved,
}

impl Id {
    /// Maximum byte length of a valid id. Picked to leave headroom
    /// under typical filesystem name limits (255 bytes on most
    /// filesystems) once the `<id>.git` suffix is appended.
    pub const MAX_LEN: usize = 240;

    /// True when `s` is a reserved name storgit uses for its own
    /// bookkeeping inside the parent tree, and therefore must not be
    /// used as an entry id.
    pub fn is_reserved(s: &str) -> bool {
        s == crate::parent::INDEX_DIR
    }

    /// Construct a validated id, returning [`Error`] on rejection.
    pub fn new(s: impl Into<String>) -> Result<Self, Error> {
        let s = s.into();
        Self::validate(&s)?;
        Ok(Id(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn validate(s: &str) -> Result<(), Error> {
        if s.is_empty() {
            return Err(Error::Empty);
        }
        if s.len() > Self::MAX_LEN {
            return Err(Error::TooLong {
                len: s.len(),
                max: Self::MAX_LEN,
            });
        }
        if s.starts_with('.') {
            return Err(Error::LeadingDot);
        }
        if s.ends_with(".git") {
            return Err(Error::GitSuffix);
        }
        if Self::is_reserved(s) {
            return Err(Error::Reserved);
        }
        for c in s.chars() {
            if c == '/' || c == '"' || c == '\\' {
                return Err(Error::BadChar(c));
            }
            let code = c as u32;
            if code < 0x20 || code == 0x7f {
                return Err(Error::BadChar(c));
            }
        }
        Ok(())
    }
}

impl std::fmt::Display for Id {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Empty => write!(f, "id is empty"),
            Error::TooLong { len, max } => write!(f, "id is {len} bytes; max is {max}"),
            Error::BadChar(c) => write!(f, "id contains forbidden character {c:?}"),
            Error::LeadingDot => write!(f, "id may not start with '.'"),
            Error::GitSuffix => write!(f, "id may not end with '.git'"),
            Error::Reserved => write!(f, "id is reserved by storgit"),
        }
    }
}

impl std::error::Error for Error {}

impl AsRef<str> for Id {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Id {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl FromStr for Id {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Id::new(s.to_string())
    }
}
