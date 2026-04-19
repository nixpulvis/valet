use crate::{encrypt::Stash, lot::Lot};
use bitcode::{Decode, Encode};
use std::{cmp::Ordering, collections::BTreeMap, fmt, str::FromStr};

/// A record's primary identifier plus optional searchable metadata.
#[derive(Encode, Decode, Debug, Eq, PartialEq, Hash, Clone)]
pub struct Label {
    /// The record's primary, exact identifier (e.g. [`LabelName::Simple`] for
    /// `"github"` or [`LabelName::Domain`] for `"nix@example.com"`). Literal
    /// [`Query`] names match this exactly, ignoring [`Label::extra`]; regex
    /// queries match against its [`Display`](fmt::Display) form.
    ///
    /// [`Query`]: crate::record::Query
    name: LabelName,
    /// Searchable key/value metadata encrypted alongside the name. Drives
    /// [`RecordIndex`] queries without ever decrypting the password-bearing
    /// [`Data`]; use it for fields like `username` or `url` that disambiguate
    /// records sharing a [`name`](Label::name).
    ///
    /// Keys and values are constrained so [`Label`]'s [`Display`](fmt::Display)
    /// is losslessly parseable back through [`Query::from_str`]: see
    /// [`Label::with_extra`] for the exact character restrictions.
    ///
    /// [`Query::from_str`]: crate::record::Query::from_str
    ///
    /// A [`BTreeMap`] so labels have a total, deterministic [`Ord`] and can be
    /// used as a [`BTreeMap`] key. Callers must not depend on the relative
    /// order of labels that differ only in their `extra` contents. Treat
    /// "labels sort by name; ties broken by extras" as an implementation
    /// detail, not an API.
    ///
    /// [`Data`]: crate::record::Data
    /// [`RecordIndex`]: crate::record::RecordIndex
    extra: BTreeMap<String, String>,
}

/// The primary, exact-identifying part of a [`Label`].
#[derive(Encode, Decode, Debug, Eq, PartialEq, Hash, Clone)]
pub enum LabelName {
    Simple(String),
    Domain { id: String, domain: String },
}

impl Stash<Lot> for Label {}

impl From<LabelName> for Label {
    fn from(name: LabelName) -> Self {
        Label {
            name,
            extra: BTreeMap::new(),
        }
    }
}

impl Label {
    /// Extras must round-trip through [`Label`]'s [`Display`](fmt::Display) and
    /// [`Query::from_str`]: keys reject whitespace, `<`, `>`, `~`, `=` and the
    /// empty string; values reject whitespace, `<`, `>`. Returns an error if a
    /// key or value is invalid.
    ///
    /// [`Query::from_str`]: crate::record::Query::from_str
    pub fn with_extra(mut self, extra: BTreeMap<String, String>) -> Result<Self, Error> {
        for (k, v) in &extra {
            validate_extra(k, v)?;
        }
        self.extra = extra;
        Ok(self)
    }

    /// See [`Label::with_extra`] for the character restrictions on `key` and
    /// `value`.
    pub fn add_extra(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<Self, Error> {
        let key = key.into();
        let value = value.into();
        validate_extra(&key, &value)?;
        self.extra.insert(key, value);
        Ok(self)
    }

    pub fn name(&self) -> &LabelName {
        &self.name
    }

    pub fn extra(&self) -> &BTreeMap<String, String> {
        &self.extra
    }
}

impl Ord for Label {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name
            .cmp(&other.name)
            .then_with(|| self.extra.cmp(&other.extra))
    }
}

impl PartialOrd for Label {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LabelName {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (LabelName::Simple(a), LabelName::Simple(b)) => a.cmp(b),
            (
                LabelName::Domain {
                    id: a_id,
                    domain: a_domain,
                },
                LabelName::Domain {
                    id: b_id,
                    domain: b_domain,
                },
            ) => a_domain.cmp(b_domain).then_with(|| a_id.cmp(b_id)),
            (LabelName::Domain { .. }, LabelName::Simple(_)) => Ordering::Less,
            (LabelName::Simple(_), LabelName::Domain { .. }) => Ordering::Greater,
        }
    }
}

impl PartialOrd for LabelName {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl FromStr for Label {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let name: LabelName = s.parse()?;
        Ok(Label {
            name,
            extra: BTreeMap::new(),
        })
    }
}

impl FromStr for LabelName {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Reject any `::`: it's reserved as the lot/label separator, so a
        // name containing it could not round-trip through a rendered
        // `lot::label` path without being re-split at the wrong boundary.
        if s.contains("::") {
            return Err(Error::InvalidName);
        }
        if let Some((id, domain)) = s.rsplit_once("@") {
            if id.trim().is_empty() {
                Err(Error::MissingId)
            } else if domain.trim().is_empty() {
                Err(Error::MissingDomain)
            } else if id.contains(is_invalid_name_char) {
                Err(Error::InvalidId)
            } else if domain.contains(is_invalid_name_char) {
                Err(Error::InvalidDomain)
            } else {
                Ok(LabelName::Domain {
                    id: id.into(),
                    domain: domain.into(),
                })
            }
        } else if s.trim().is_empty() {
            Err(Error::MissingId)
        } else if s.contains(is_invalid_name_char) {
            Err(Error::InvalidName)
        } else {
            Ok(LabelName::Simple(s.into()))
        }
    }
}

/// Characters forbidden anywhere in a label name. Whitespace breaks token
/// boundaries; `<` / `>` collide with the extras grammar used by queries;
/// `~` is the regex / regex-filter marker.
fn is_invalid_name_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '>' | '~')
}

fn validate_extra(key: &str, value: &str) -> Result<(), Error> {
    if key.is_empty() || key.contains(is_invalid_extra_key_char) {
        Err(Error::InvalidExtraKey)
    } else if value.contains(is_invalid_extra_value_char) {
        Err(Error::InvalidExtraValue)
    } else {
        Ok(())
    }
}

/// Keys are whitespace-separated tokens in the query grammar, so they cannot
/// contain whitespace or the block delimiters `<`/`>`. They also cannot contain
/// `=` or `~`, which mark the key/value separator, or a leading/embedded `~`
/// would flip the key into regex mode at parse time.
fn is_invalid_extra_key_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '>' | '~' | '=')
}

/// Values terminate at whitespace or the closing `>` of the extras block, so
/// neither can appear literally. `<` would be read as a nested block opener by
/// `split_name_extras`, which scans for the last `<` before the trailing `>`.
fn is_invalid_extra_value_char(c: char) -> bool {
    c.is_whitespace() || matches!(c, '<' | '>')
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if !self.extra.is_empty() {
            write!(f, "<")?;
            let mut first = true;
            for (k, v) in &self.extra {
                if !first {
                    write!(f, " ")?;
                }
                first = false;
                write!(f, "{k}={v}")?;
            }
            write!(f, ">")?;
        }
        Ok(())
    }
}

impl fmt::Display for LabelName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LabelName::Simple(s) => write!(f, "{s}"),
            LabelName::Domain { id, domain } => write!(f, "{id}@{domain}"),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidName,
    MissingId,
    InvalidId,
    MissingDomain,
    InvalidDomain,
    InvalidExtraKey,
    InvalidExtraValue,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidName => write!(f, "invalid name"),
            Error::MissingId => write!(f, "missing id"),
            Error::InvalidId => write!(f, "invalid id"),
            Error::MissingDomain => write!(f, "missing domain"),
            Error::InvalidDomain => write!(f, "invalid domain"),
            Error::InvalidExtraKey => write!(f, "invalid extras key"),
            Error::InvalidExtraValue => write!(f, "invalid extras value"),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lot::Lot;

    #[test]
    fn encode_decode() {
        let label = "foo".parse::<Label>().unwrap();
        let encoded = label.encode();
        let decoded = Label::decode(&encoded).expect("failed to decode");
        assert_eq!(label, decoded);
    }

    #[test]
    fn encrypt_decrypt() {
        let lot = Lot::new("test");
        let label = "foo".parse::<Label>().unwrap();
        let encrypted = label.encrypt(lot.key()).expect("failed to encrypt");
        let decrypted = Label::decrypt(&encrypted, lot.key()).expect("failed to decrypt");
        assert_eq!(label, decrypted);
    }

    #[test]
    fn encrypt_decrypt_with_aad() {
        let lot = Lot::new("test");
        let label = "foo".parse::<Label>().unwrap();
        let aad = [1, 2, 3];
        let encrypted = label
            .encrypt_with_aad(lot.key(), &aad)
            .expect("failed to encrypt");
        let decrypted =
            Label::decrypt_with_aad(&encrypted, lot.key(), &aad).expect("failed to decrypt");
        assert_eq!(label, decrypted);
    }

    #[test]
    fn ord_domain_sorts_by_domain_then_id() {
        let mut labels = vec![
            "bob@example.com".parse::<Label>().unwrap(),
            "alice@zeta.com".parse::<Label>().unwrap(),
            "alice@example.com".parse::<Label>().unwrap(),
        ];
        labels.sort();
        assert_eq!(
            labels,
            vec![
                "alice@example.com".parse::<Label>().unwrap(),
                "bob@example.com".parse::<Label>().unwrap(),
                "alice@zeta.com".parse::<Label>().unwrap(),
            ]
        );
    }

    #[test]
    fn ord_extras_break_ties_totally() {
        // Same name, different extras: must be a total (non-Equal) ordering so
        // both can coexist as BTreeMap keys.
        let a = "foo"
            .parse::<Label>()
            .unwrap()
            .add_extra("user", "alice")
            .unwrap();
        let b = "foo"
            .parse::<Label>()
            .unwrap()
            .add_extra("user", "bob")
            .unwrap();
        assert_ne!(a.cmp(&b), Ordering::Equal);
        // Empty extras compare less than a populated one under BTreeMap's
        // lexicographic ordering; that's the invariant we need.
        let empty = "foo".parse::<Label>().unwrap();
        assert_eq!(empty.cmp(&a), Ordering::Less);
    }

    #[test]
    fn ord_names_outrank_extras() {
        // Differences in `name` always dominate differences in `extra`.
        let a = "a".parse::<Label>().unwrap().add_extra("z", "z").unwrap();
        let b = "b".parse::<Label>().unwrap();
        assert_eq!(a.cmp(&b), Ordering::Less);
    }

    #[test]
    fn display_includes_extras() {
        let label = "foo".parse::<Label>().unwrap();
        assert_eq!(label.to_string(), "foo");
        let label = "foo"
            .parse::<Label>()
            .unwrap()
            .add_extra("username", "nix")
            .unwrap();
        assert_eq!(label.to_string(), "foo<username=nix>");
        let label = "nix@example.com"
            .parse::<Label>()
            .unwrap()
            .add_extra("url", "github.com")
            .unwrap()
            .add_extra("username", "nix")
            .unwrap();
        assert_eq!(
            label.to_string(),
            "nix@example.com<url=github.com username=nix>"
        );
    }

    #[test]
    fn encode_decode_preserves_extras() {
        let label = "nix@example.com"
            .parse::<Label>()
            .unwrap()
            .add_extra("username", "nix")
            .unwrap()
            .add_extra("url", "github.com")
            .unwrap();
        let decoded = Label::decode(&label.encode()).expect("failed to decode");
        assert_eq!(label, decoded);
        assert_eq!(decoded.extra().len(), 2);
    }

    #[test]
    fn parse_simple() {
        let label = "label".parse::<Label>().unwrap();
        assert_eq!(label, "label".parse::<Label>().unwrap());
        assert!(label.extra().is_empty());
    }

    #[test]
    fn parse_rejects_query_metachars() {
        // `<` / `>` / `~` all collide with the query grammar, so labels
        // must not carry them regardless of position.
        assert_eq!("foo<bar".parse::<Label>(), Err(Error::InvalidName));
        assert_eq!("foo>bar".parse::<Label>(), Err(Error::InvalidName));
        assert_eq!("foo~bar".parse::<Label>(), Err(Error::InvalidName));
        assert_eq!("ni<x@example.com".parse::<Label>(), Err(Error::InvalidId));
        assert_eq!("ni~x@example.com".parse::<Label>(), Err(Error::InvalidId));
        assert_eq!(
            "nix@exa>mple.com".parse::<Label>(),
            Err(Error::InvalidDomain)
        );
        assert_eq!(
            "nix@exa~mple.com".parse::<Label>(),
            Err(Error::InvalidDomain)
        );
    }

    #[test]
    fn parse_rejects_lot_separator() {
        // `::` anywhere would re-split at the wrong boundary when rendered
        // back as `lot::label`, so LabelName rejects it outright.
        assert_eq!("::foo".parse::<Label>(), Err(Error::InvalidName));
        assert_eq!("::".parse::<Label>(), Err(Error::InvalidName));
        assert_eq!("foo::bar".parse::<Label>(), Err(Error::InvalidName));
        assert_eq!("foo::".parse::<Label>(), Err(Error::InvalidName));
    }

    #[test]
    fn extras_reject_grammar_chars() {
        let base = || "foo".parse::<Label>().unwrap();
        // Keys reject whitespace, <, >, ~, =, and emptiness.
        assert_eq!(base().add_extra("", "v"), Err(Error::InvalidExtraKey));
        assert_eq!(base().add_extra("a b", "v"), Err(Error::InvalidExtraKey));
        assert_eq!(base().add_extra("a<b", "v"), Err(Error::InvalidExtraKey));
        assert_eq!(base().add_extra("a>b", "v"), Err(Error::InvalidExtraKey));
        assert_eq!(base().add_extra("a~b", "v"), Err(Error::InvalidExtraKey));
        assert_eq!(base().add_extra("a=b", "v"), Err(Error::InvalidExtraKey));
        // Values reject whitespace, <, >. `=` and `~` are OK in values: the
        // filter parser commits to the first separator and treats the rest
        // literally.
        assert_eq!(base().add_extra("k", "a b"), Err(Error::InvalidExtraValue));
        assert_eq!(base().add_extra("k", "a<b"), Err(Error::InvalidExtraValue));
        assert_eq!(base().add_extra("k", "a>b"), Err(Error::InvalidExtraValue));
        assert!(base().add_extra("k", "a=b").is_ok());
        assert!(base().add_extra("k", "a~b").is_ok());
    }

    #[test]
    fn parse_domain() {
        let username = "user@example.com".parse::<Label>().unwrap();
        assert!(
            matches!(&username.name, LabelName::Domain { id, domain } if id == "user" && domain == "example.com")
        );
        assert!(username.extra().is_empty());
        let email = "user@email.com@example.com".parse::<Label>().unwrap();
        assert!(
            matches!(&email.name, LabelName::Domain { id, domain } if id == "user@email.com" && domain == "example.com")
        );

        assert_eq!("".parse::<Label>(), Err(Error::MissingId));
        assert_eq!(" \t".parse::<Label>(), Err(Error::MissingId));
        assert_eq!(" @\t".parse::<Label>(), Err(Error::MissingId));
        assert_eq!("user@".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user@\t".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user@@".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user@@ ".parse::<Label>(), Err(Error::MissingDomain));
        assert_eq!("user @email.com".parse::<Label>(), Err(Error::InvalidId));
        assert_eq!(
            "user@\temail.com".parse::<Label>(),
            Err(Error::InvalidDomain)
        );
        assert_eq!(
            "user @email.com@example.com".parse::<Label>(),
            Err(Error::InvalidId)
        );
        assert_eq!(
            "user@ email.com@example.com".parse::<Label>(),
            Err(Error::InvalidId)
        );
    }
}
