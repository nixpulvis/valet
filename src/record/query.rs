use crate::{
    lot::DEFAULT_LOT,
    record::{Label, LabelName, label},
};
use regex::Regex;
use std::{collections::BTreeMap, fmt, str::FromStr};

/// A search query over the labels in one or more lots.
///
/// Grammar:
///
/// ```text
/// (<lot-spec>::)?<name-spec><extras-spec>?
/// ```
///
/// - `<lot-spec>` (optional; split at the *last* `::`):
///   - absent or empty: the query targets the default lot (`main`) as a
///     literal. To search across all lots, write an explicit match-all lot
///     regex: `~::...`.
///   - literal: `lot-name`. Matches a lot name exactly.
///   - regex: `~pattern`. Matches when `pattern` matches the lot name. Use
///     `^...$` to anchor.
/// - `<name-spec>`:
///   - empty (e.g. `main::` or `main::<k=v>`): match every label in the lot.
///     Any trailing extras still apply.
///   - literal: `foo` or `id@domain`. Matches [`LabelName`] exactly, ignoring
///     [`Label::extra`] ("mostly-exact").
///   - regex: `~pattern`. Matches when `pattern` matches the [`LabelName`]
///     display form. Use `^...$` to anchor.
/// - `<extras-spec>` (optional, trailing, AND-composed across
///   whitespace-separated filters, wrapped in `<...>`). Extras keys and values
///   forbid whitespace (see [`Label::add_extra`]), so the split is
///   unambiguous:
///   - key mode: a leading `~` on a filter marks the key as a regex
///     (`<~key...>`); otherwise the key is a literal.
///   - value mode: `=value` is a literal value, `~pattern` is a regex value,
///     absent means presence-only (any value satisfies).
///
/// Label names forbid `<` and `>`, so a trailing `<...>` in a query is
/// unambiguously the extras block.
///
/// So the six filter shapes are:
///
/// | filter         | meaning                                              |
/// |----------------|------------------------------------------------------|
/// | `<k>`          | literal key `k` is present                           |
/// | `<~k>`         | some key matches regex `k`                           |
/// | `<k=v>`        | `extra[k] == v`                                      |
/// | `<k~p>`        | `extra[k]` matches regex `p`                         |
/// | `<~k=v>`       | some key matching `k` has value equal to `v`         |
/// | `<~k~p>`       | some key matching `k` has a value matching `p`       |
pub struct Query {
    lot: LotMatch,
    name: NameMatch,
    extras: Vec<ExtraFilter>,
}

/// A fully-literal query target: a specific lot name plus a specific label.
///
/// Produced by [`Query::into_path`]; used by callers like the CLI `put` path
/// that need an exact storage target rather than a search predicate.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Path {
    pub lot: String,
    pub label: Label,
}

/// Match mode for a lot name.
enum LotMatch {
    /// Exact match on the lot name.
    Literal(String),
    /// Regex against the lot name.
    Regex(Regex),
}

/// Match mode for a [`Label`]'s [`LabelName`].
enum NameMatch {
    /// Exact match on the full [`LabelName`]; [`Label::extra`] is ignored.
    Literal(LabelName),
    /// Regex against the [`LabelName`]'s display form.
    Regex(Regex),
}

/// One constraint on [`Label::extra`]. Matches if some `(key, value)` entry
/// satisfies both the [`KeyMatch`] and the [`ValueMatch`] (if any).
struct ExtraFilter {
    key: KeyMatch,
    /// `None` is a presence-only check; any value satisfies.
    value: Option<ValueMatch>,
}

enum KeyMatch {
    Literal(String),
    Regex(Regex),
}

enum ValueMatch {
    Eq(String),
    Regex(Regex),
}

impl Query {
    /// Build a query that name-prefix matches `s` as a literal string, with no
    /// extras filter. `s` is regex-escaped so metacharacters match literally;
    /// callers that want full regex semantics should use [`Query::from_str`]
    /// with a leading `~` instead.
    ///
    /// Only the label side is constrained: the lot spec is match-all, so
    /// [`Query::matches_lot`] accepts every lot name. Callers that only call
    /// [`Query::matches_label`] (e.g. when they already have the lot in hand)
    /// are unaffected; callers combining this with [`Query::matches_lot`]
    /// across multiple lots will see cross-lot hits by design.
    pub fn label_prefix(s: &str, case_insensitive: bool) -> Self {
        let escaped = regex::escape(s);
        let pattern = if case_insensitive {
            format!("(?i)^{escaped}")
        } else {
            format!("^{escaped}")
        };
        Query {
            lot: LotMatch::Regex(Regex::new("").expect("empty regex is valid")),
            name: NameMatch::Regex(Regex::new(&pattern).expect("escaped regex is valid")),
            extras: Vec::new(),
        }
    }

    /// True if `lot_name` satisfies this query's lot spec.
    pub fn matches_lot(&self, lot_name: &str) -> bool {
        match &self.lot {
            LotMatch::Literal(want) => want == lot_name,
            LotMatch::Regex(re) => re.is_match(lot_name),
        }
    }

    /// True if `label` satisfies this query's name + extras spec.
    ///
    /// Does *not* check the lot spec; use [`Query::matches_lot`] for that.
    pub fn matches_label(&self, label: &Label) -> bool {
        let name_ok = match &self.name {
            NameMatch::Literal(want) => label.name() == want,
            NameMatch::Regex(re) => re.is_match(&label.name().to_string()),
        };
        name_ok && self.extras.iter().all(|f| f.matches(label))
    }

    /// Collapse a fully-literal query into the [`Path`] it addresses.
    ///
    /// Used by `put`, which needs a specific storage target. Returns
    /// [`Error::NotLiteral`] if the query uses a regex on the lot, name, or in
    /// any extras filter, or if any extras filter is presence-only. Returns
    /// [`Error::DuplicateKey`] if the same key appears more than once.
    pub fn into_path(self) -> Result<Path, Error> {
        let Query { lot, name, extras } = self;
        let lot = match lot {
            LotMatch::Literal(l) => l,
            LotMatch::Regex(_) => return Err(Error::NotLiteral),
        };
        let name = match name {
            NameMatch::Literal(n) => n,
            NameMatch::Regex(_) => return Err(Error::NotLiteral),
        };
        let mut extra = BTreeMap::new();
        for f in extras {
            let key = match f.key {
                KeyMatch::Literal(k) => k,
                KeyMatch::Regex(_) => return Err(Error::NotLiteral),
            };
            let value = match f.value {
                Some(ValueMatch::Eq(v)) => v,
                Some(ValueMatch::Regex(_)) | None => return Err(Error::NotLiteral),
            };
            if extra.insert(key.clone(), value).is_some() {
                return Err(Error::DuplicateKey(key));
            }
        }
        let label = Label::from(name).with_extra(extra)?;
        Ok(Path { lot, label })
    }
}

impl Path {
    pub fn new(lot: impl Into<String>, label: Label) -> Self {
        Path {
            lot: lot.into(),
            label,
        }
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.lot, self.label)
    }
}

impl fmt::Display for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.lot {
            LotMatch::Literal(l) => write!(f, "{l}")?,
            LotMatch::Regex(re) => write!(f, "~{}", re.as_str())?,
        }
        write!(f, "::")?;
        match &self.name {
            NameMatch::Literal(n) => write!(f, "{n}")?,
            NameMatch::Regex(re) => write!(f, "~{}", re.as_str())?,
        }
        if !self.extras.is_empty() {
            write!(f, "<")?;
            let mut first = true;
            for filter in &self.extras {
                if !first {
                    write!(f, " ")?;
                }
                first = false;
                filter.fmt(f)?;
            }
            write!(f, ">")?;
        }
        Ok(())
    }
}

impl fmt::Display for ExtraFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.key {
            KeyMatch::Literal(k) => write!(f, "{k}")?,
            KeyMatch::Regex(re) => write!(f, "~{}", re.as_str())?,
        }
        match &self.value {
            None => Ok(()),
            Some(ValueMatch::Eq(v)) => write!(f, "={v}"),
            Some(ValueMatch::Regex(re)) => write!(f, "~{}", re.as_str()),
        }
    }
}

impl ExtraFilter {
    pub fn matches(&self, label: &Label) -> bool {
        label.extra().iter().any(|(k, v)| {
            let key_ok = match &self.key {
                KeyMatch::Literal(lit) => lit == k,
                KeyMatch::Regex(re) => re.is_match(k),
            };
            if !key_ok {
                return false;
            }
            match &self.value {
                None => true,
                Some(ValueMatch::Eq(want)) => want == v,
                Some(ValueMatch::Regex(re)) => re.is_match(v),
            }
        })
    }
}

impl FromStr for Query {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (lot_str, rest) = split_lot(s);
        let lot = parse_lot(lot_str)?;
        let (name_str, extras_str) = split_name_extras(rest)?;
        let name = parse_name(name_str)?;
        let extras = match extras_str {
            None => Vec::new(),
            Some(inner) => parse_extras(inner)?,
        };
        Ok(Query { lot, name, extras })
    }
}

/// Split an input string into `(lot-spec, rest)`.
///
/// The lot prefix is separated from the name + extras by the *last* `::` in
/// the string. If there is no `::`, the whole input is the name + extras and
/// the lot prefix is absent. Splitting on the last `::` lets lot names
/// themselves contain `::` (e.g. `team::eng::github`).
fn split_lot(s: &str) -> (Option<&str>, &str) {
    match s.rfind("::") {
        Some(idx) => (Some(&s[..idx]), &s[idx + 2..]),
        None => (None, s),
    }
}

fn parse_lot(s: Option<&str>) -> Result<LotMatch, Error> {
    match s.unwrap_or("") {
        "" => Ok(LotMatch::Literal(DEFAULT_LOT.to_string())),
        spec => match spec.strip_prefix('~') {
            Some(pat) => Ok(LotMatch::Regex(Regex::new(pat)?)),
            None => Ok(LotMatch::Literal(spec.to_string())),
        },
    }
}

/// Split a query string into `(name, extras?)`.
///
/// The extras block is optional and, if present, always a trailing `<...>`.
/// A regex name may legitimately contain `<` (e.g. `~a<b.*`), so we can't
/// just split on the first `<`. The extras block is recognized only when `s`
/// ends with `>`, and within that case we take the *last* `<` as the opener
/// so that stray `<`s inside a regex name stay with the name.
///
/// Nested `<...>` aren't part of the grammar: labels forbid `<`/`>` and
/// extras values do too (see [`Label::with_extra`]), so no well-formed
/// label's [`Display`](fmt::Display) produces nested brackets.
fn split_name_extras(s: &str) -> Result<(&str, Option<&str>), Error> {
    if !s.ends_with('>') {
        return Ok((s, None));
    }
    let without_close = &s[..s.len() - 1];
    let Some(lt) = without_close.rfind('<') else {
        return Err(Error::UnclosedAngle);
    };
    Ok((&s[..lt], Some(&s[lt + 1..s.len() - 1])))
}

fn parse_name(s: &str) -> Result<NameMatch, Error> {
    if s.is_empty() {
        // An empty name slot (e.g. `main::` or `main::<k=v>`) is the
        // "all labels" shortcut: match every name, then let any trailing
        // extras filter further. An empty regex matches at every position,
        // so it's a natural match-all.
        return Ok(NameMatch::Regex(
            Regex::new("").expect("empty regex is valid"),
        ));
    }
    if let Some(rest) = s.strip_prefix('~') {
        Ok(NameMatch::Regex(Regex::new(rest)?))
    } else {
        // A bare `<` in a literal name means the user opened an extras block
        // without closing it. Reject instead of letting `foo<` parse as the
        // simple label "foo<".
        if s.contains('<') {
            return Err(Error::UnclosedAngle);
        }
        Ok(NameMatch::Literal(LabelName::from_str(s)?))
    }
}

fn parse_extras(s: &str) -> Result<Vec<ExtraFilter>, Error> {
    s.split_whitespace().map(parse_filter).collect()
}

fn parse_filter(s: &str) -> Result<ExtraFilter, Error> {
    let (key_is_regex, rest) = match s.strip_prefix('~') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let eq = rest.find('=');
    let ti = rest.find('~');
    let (sep_idx, value_is_regex) = match (eq, ti) {
        (None, None) => {
            let key_str = rest.trim();
            if key_str.is_empty() {
                return Err(Error::EmptyFilterKey);
            }
            let key = build_key(key_is_regex, key_str)?;
            return Ok(ExtraFilter { key, value: None });
        }
        (Some(i), None) => (i, false),
        (None, Some(i)) => (i, true),
        (Some(e), Some(t)) => {
            if e < t {
                (e, false)
            } else {
                (t, true)
            }
        }
    };
    let key_str = rest[..sep_idx].trim();
    if key_str.is_empty() {
        return Err(Error::EmptyFilterKey);
    }
    let value_str = &rest[sep_idx + 1..];
    let key = build_key(key_is_regex, key_str)?;
    let value = if value_is_regex {
        if value_str.is_empty() {
            return Err(Error::EmptyFilterValueRegex);
        }
        Some(ValueMatch::Regex(Regex::new(value_str)?))
    } else {
        Some(ValueMatch::Eq(value_str.to_string()))
    };
    Ok(ExtraFilter { key, value })
}

fn build_key(is_regex: bool, s: &str) -> Result<KeyMatch, Error> {
    if is_regex {
        Ok(KeyMatch::Regex(Regex::new(s)?))
    } else {
        Ok(KeyMatch::Literal(s.to_string()))
    }
}

#[derive(Debug, PartialEq)]
pub enum Error {
    UnclosedAngle,
    EmptyFilterKey,
    EmptyFilterValueRegex,
    NotLiteral,
    DuplicateKey(String),
    Name(label::Error),
    Regex(regex::Error),
}

impl From<label::Error> for Error {
    fn from(e: label::Error) -> Self {
        Error::Name(e)
    }
}

impl From<regex::Error> for Error {
    fn from(e: regex::Error) -> Self {
        Error::Regex(e)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::UnclosedAngle => write!(f, "unclosed '<' in query"),
            Error::EmptyFilterKey => write!(f, "extras filter has empty key"),
            Error::EmptyFilterValueRegex => write!(f, "extras filter `~` has empty value regex"),
            Error::NotLiteral => {
                write!(
                    f,
                    "query must be literal (no regex or presence-only extras)"
                )
            }
            Error::DuplicateKey(k) => write!(f, "duplicate extras key: {k}"),
            Error::Name(e) => write!(f, "invalid label name: {e}"),
            Error::Regex(e) => write!(f, "invalid regex: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Query {
        s.parse::<Query>()
            .unwrap_or_else(|e| panic!("parse {s:?}: {e}"))
    }

    #[test]
    fn parse_literal_simple() {
        let q = p("foo");
        assert!(matches!(&q.name, NameMatch::Literal(LabelName::Simple(s)) if s == "foo"));
        assert!(q.extras.is_empty());
    }

    #[test]
    fn parse_literal_domain() {
        let q = p("nix@example.com");
        assert!(matches!(
            &q.name,
            NameMatch::Literal(LabelName::Domain { id, domain })
                if id == "nix" && domain == "example.com"
        ));
    }

    #[test]
    fn parse_regex_only() {
        let q = p("~nix@.*");
        let NameMatch::Regex(re) = &q.name else {
            panic!("expected regex");
        };
        assert!(re.is_match("nix@example.com"));
    }

    #[test]
    fn parse_literal_key_eq() {
        let q = p("foo<username=nix>");
        assert_eq!(q.extras.len(), 1);
        let f = &q.extras[0];
        assert!(matches!(&f.key, KeyMatch::Literal(k) if k == "username"));
        assert!(matches!(&f.value, Some(ValueMatch::Eq(v)) if v == "nix"));
    }

    #[test]
    fn parse_literal_key_regex_value() {
        let q = p("foo<url~github>");
        let f = &q.extras[0];
        assert!(matches!(&f.key, KeyMatch::Literal(k) if k == "url"));
        assert!(matches!(&f.value, Some(ValueMatch::Regex(_))));
    }

    #[test]
    fn parse_literal_key_presence() {
        let q = p("foo<username>");
        let f = &q.extras[0];
        assert!(matches!(&f.key, KeyMatch::Literal(k) if k == "username"));
        assert!(f.value.is_none());
    }

    #[test]
    fn parse_regex_key_presence() {
        let q = p("foo<~user.*>");
        let f = &q.extras[0];
        assert!(matches!(&f.key, KeyMatch::Regex(_)));
        assert!(f.value.is_none());
    }

    #[test]
    fn parse_regex_key_eq_value() {
        let q = p("foo<~user.*=nix>");
        let f = &q.extras[0];
        assert!(matches!(&f.key, KeyMatch::Regex(_)));
        assert!(matches!(&f.value, Some(ValueMatch::Eq(v)) if v == "nix"));
    }

    #[test]
    fn parse_regex_key_regex_value() {
        let q = p("foo<~user.*~^nix$>");
        let f = &q.extras[0];
        assert!(matches!(&f.key, KeyMatch::Regex(_)));
        assert!(matches!(&f.value, Some(ValueMatch::Regex(_))));
    }

    #[test]
    fn parse_multiple_extras() {
        let q = p("nix@ex.com<username=nix url~github>");
        assert_eq!(q.extras.len(), 2);
    }

    #[test]
    fn parse_regex_name_with_extras() {
        let q = p("~.*@example.com<username=nix>");
        assert!(matches!(&q.name, NameMatch::Regex(_)));
        assert_eq!(q.extras.len(), 1);
    }

    #[test]
    fn parse_errors() {
        fn err(s: &str) -> Error {
            s.parse::<Query>().err().unwrap()
        }
        assert_eq!(err("foo<k=v"), Error::UnclosedAngle);
        assert_eq!(err("foo<"), Error::UnclosedAngle);
        assert_eq!(err("foo<=v>"), Error::EmptyFilterKey);
        assert_eq!(err("foo<k~>"), Error::EmptyFilterValueRegex);
    }

    #[test]
    fn bare_tilde_is_match_all() {
        // Empty regex matches every position, so a bare `~` in the top-level
        // lot or name slot is the natural match-all. (In an extras filter
        // value it's rejected as EmptyFilterValueRegex; see parse_errors.)
        let q = p("~");
        assert!(q.matches_label(&"anything".parse::<Label>().unwrap()));
        let q = p("~::");
        assert!(q.matches_lot("any"));
        assert!(q.matches_label(&"x".parse::<Label>().unwrap()));
    }

    #[test]
    fn prefix_escapes_metacharacters() {
        let q = Query::label_prefix("foo.com", true);
        assert!(q.matches_label(&"foo.com".parse::<Label>().unwrap()));
        assert!(q.matches_label(&"foo.com.extra".parse::<Label>().unwrap()));
        // The `.` is literal, not a regex wildcard.
        assert!(!q.matches_label(&"fooXcom".parse::<Label>().unwrap()));
    }

    #[test]
    fn prefix_case_insensitive() {
        let q = Query::label_prefix("GitHub", true);
        assert!(q.matches_label(&"github".parse::<Label>().unwrap()));
        assert!(q.matches_label(&"GITHUB.com".parse::<Label>().unwrap()));
    }

    #[test]
    fn prefix_case_sensitive() {
        let q = Query::label_prefix("GitHub", false);
        assert!(q.matches_label(&"GitHub".parse::<Label>().unwrap()));
        assert!(!q.matches_label(&"github".parse::<Label>().unwrap()));
    }

    #[test]
    fn prefix_matches_any_lot() {
        // Query::label_prefix is built for GUI autocomplete, which has the lot in
        // hand already. Its lot spec must accept every lot name so callers
        // that do combine it with matches_lot get cross-lot hits by design.
        let q = Query::label_prefix("foo", true);
        assert!(q.matches_lot("main"));
        assert!(q.matches_lot("work"));
        assert!(q.matches_lot(""));
        assert!(q.matches_lot("team::eng"));
    }

    #[test]
    fn prefix_anchors_to_start() {
        let q = Query::label_prefix("hub", true);
        assert!(q.matches_label(&"hub.com".parse::<Label>().unwrap()));
        assert!(!q.matches_label(&"github".parse::<Label>().unwrap()));
    }

    #[test]
    fn matches_literal_ignores_extras() {
        let q = p("foo");
        let label = "foo"
            .parse::<Label>()
            .unwrap()
            .add_extra("anything", "goes")
            .unwrap();
        assert!(q.matches_label(&label));
        assert!(!q.matches_label(&"bar".parse::<Label>().unwrap()));
    }

    #[test]
    fn matches_regex_against_display() {
        let q = p("~.*@example\\.com");
        assert!(q.matches_label(&"nix@example.com".parse::<Label>().unwrap()));
        assert!(!q.matches_label(&"nix@other.com".parse::<Label>().unwrap()));
    }

    #[test]
    fn matches_extras_eq() {
        let q = p("~.*<username=nix>");
        assert!(
            q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("username", "nix")
                    .unwrap()
            )
        );
        assert!(
            !q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("username", "alt")
                    .unwrap()
            )
        );
        assert!(!q.matches_label(&"x".parse::<Label>().unwrap()));
    }

    #[test]
    fn matches_extras_regex_value() {
        let q = p("~.*<url~github\\.com>");
        assert!(
            q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("url", "https://github.com/nixpulvis")
                    .unwrap()
            )
        );
    }

    #[test]
    fn matches_literal_key_presence() {
        let q = p("~.*<username>");
        assert!(
            q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("username", "anything")
                    .unwrap()
            )
        );
        assert!(!q.matches_label(&"x".parse::<Label>().unwrap().add_extra("url", "x").unwrap()));
    }

    #[test]
    fn matches_regex_key_presence() {
        // Any key starting with "user".
        let q = p("~.*<~^user>");
        assert!(
            q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("username", "nix")
                    .unwrap()
            )
        );
        assert!(
            q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("user_id", "42")
                    .unwrap()
            )
        );
        assert!(
            !q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("email", "x@y")
                    .unwrap()
            )
        );
    }

    #[test]
    fn matches_regex_key_with_value_filter() {
        // "Some key matches ^u AND has value 'nix'" (any-semantics across
        // matching keys).
        let q = p("~.*<~^u=nix>");
        assert!(
            q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("username", "nix")
                    .unwrap()
            )
        );
        assert!(
            !q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("username", "alt")
                    .unwrap()
            )
        );
        // `url` also matches `^u` and has value "nix", so the label as a
        // whole satisfies the filter even though `username` doesn't.
        assert!(
            q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("username", "alt")
                    .unwrap()
                    .add_extra("url", "nix")
                    .unwrap()
            )
        );
        // No key matches `^u` at all: doesn't satisfy.
        assert!(
            !q.matches_label(
                &"x".parse::<Label>()
                    .unwrap()
                    .add_extra("email", "nix")
                    .unwrap()
            )
        );
    }

    #[test]
    fn matches_combined_and_semantics() {
        let q = p("~.*@example\\.com<username=nix>");
        let ok = "nix@example.com"
            .parse::<Label>()
            .unwrap()
            .add_extra("username", "nix")
            .unwrap();
        let wrong_domain = "nix@other.com"
            .parse::<Label>()
            .unwrap()
            .add_extra("username", "nix")
            .unwrap();
        let wrong_user = "nix@example.com"
            .parse::<Label>()
            .unwrap()
            .add_extra("username", "alt")
            .unwrap();
        assert!(q.matches_label(&ok));
        assert!(!q.matches_label(&wrong_domain));
        assert!(!q.matches_label(&wrong_user));
    }

    #[test]
    fn into_path_rejects_fuzzy() {
        assert!(matches!(p("~foo").into_path(), Err(Error::NotLiteral)));
        assert!(matches!(p("foo<k~v>").into_path(), Err(Error::NotLiteral)));
        assert!(matches!(p("foo<~k=v>").into_path(), Err(Error::NotLiteral)));
        assert!(matches!(p("foo<k>").into_path(), Err(Error::NotLiteral)));
        assert!(matches!(
            p("~main::foo").into_path(),
            Err(Error::NotLiteral)
        ));
    }

    #[test]
    fn into_path_rejects_duplicate_keys() {
        let err = p("foo<k=v k=w>").into_path().unwrap_err();
        assert!(matches!(err, Error::DuplicateKey(k) if k == "k"));
    }

    #[test]
    fn into_path_builds_literal() {
        let path = p("work::nix@example.com<username=nix url=github.com>")
            .into_path()
            .unwrap();
        assert_eq!(path.lot, "work");
        assert_eq!(
            path.label.name(),
            &LabelName::Domain {
                id: "nix".into(),
                domain: "example.com".into(),
            }
        );
        assert_eq!(
            path.label.extra().get("username").map(String::as_str),
            Some("nix")
        );
        assert_eq!(
            path.label.extra().get("url").map(String::as_str),
            Some("github.com")
        );
    }

    #[test]
    fn parse_default_lot_when_absent() {
        let q = p("foo");
        assert!(matches!(&q.lot, LotMatch::Literal(l) if l == DEFAULT_LOT));
    }

    #[test]
    fn parse_default_lot_when_empty_prefix() {
        let q = p("::foo");
        assert!(matches!(&q.lot, LotMatch::Literal(l) if l == DEFAULT_LOT));
        assert!(matches!(&q.name, NameMatch::Literal(LabelName::Simple(s)) if s == "foo"));
    }

    #[test]
    fn parse_lot_literal() {
        let q = p("work::foo");
        assert!(matches!(&q.lot, LotMatch::Literal(l) if l == "work"));
    }

    #[test]
    fn parse_lot_nested_literal() {
        // Last `::` separates lot from label; earlier `::` stays in the lot.
        let q = p("team::eng::foo");
        assert!(matches!(&q.lot, LotMatch::Literal(l) if l == "team::eng"));
        assert!(matches!(&q.name, NameMatch::Literal(LabelName::Simple(s)) if s == "foo"));
    }

    #[test]
    fn parse_lot_regex() {
        let q = p("~work.*::foo");
        assert!(matches!(&q.lot, LotMatch::Regex(_)));
        assert!(q.matches_lot("work"));
        assert!(q.matches_lot("workshop"));
        assert!(!q.matches_lot("home"));
    }

    #[test]
    fn parse_lot_and_name_regex() {
        let q = p("~work.*::~foo.*");
        assert!(matches!(&q.lot, LotMatch::Regex(_)));
        assert!(matches!(&q.name, NameMatch::Regex(_)));
        assert!(q.matches_lot("workshop"));
        assert!(q.matches_label(&"football".parse::<Label>().unwrap()));
    }

    #[test]
    fn parse_empty_lot_regex_is_match_all() {
        // `~` with nothing after (as the lot spec) = any lot.
        let q = p("~::foo");
        assert!(matches!(&q.lot, LotMatch::Regex(_)));
        assert!(q.matches_lot("anything"));
        assert!(q.matches_lot("main"));
    }

    #[test]
    fn parse_empty_name_is_match_all() {
        // `lot::` with nothing after `::` = every label in that lot.
        let q = p("main::");
        assert!(matches!(&q.lot, LotMatch::Literal(l) if l == "main"));
        assert!(matches!(&q.name, NameMatch::Regex(_)));
        assert!(q.matches_label(&"anything".parse::<Label>().unwrap()));
        assert!(q.matches_label(&"nix@example.com".parse::<Label>().unwrap()));
    }

    #[test]
    fn parse_empty_name_with_extras() {
        // `lot::<k=v>` = every label in that lot with extras filter applied.
        let q = p("main::<username=nix>");
        assert!(matches!(&q.lot, LotMatch::Literal(l) if l == "main"));
        assert_eq!(q.extras.len(), 1);
        let label = "anything"
            .parse::<Label>()
            .unwrap()
            .add_extra("username", "nix")
            .unwrap();
        assert!(q.matches_label(&label));
        assert!(!q.matches_label(&"anything".parse::<Label>().unwrap()));
    }

    #[test]
    fn parse_regex_lot_empty_name() {
        let q = p("~m.*::");
        assert!(matches!(&q.lot, LotMatch::Regex(_)));
        assert!(matches!(&q.name, NameMatch::Regex(_)));
        assert!(q.matches_lot("main"));
        assert!(q.matches_label(&"anything".parse::<Label>().unwrap()));
    }

    #[test]
    fn matches_lot_only_checks_lot() {
        let q = p("work::foo");
        assert!(q.matches_lot("work"));
        assert!(!q.matches_lot("home"));
    }

    #[test]
    fn display_roundtrips_literal() {
        let s = "work::nix@example.com<url=https://example.com username=nix>";
        assert_eq!(p(s).to_string(), s);
    }

    #[test]
    fn display_regex_forms() {
        assert_eq!(p("~w.*::foo").to_string(), "~w.*::foo");
        assert_eq!(p("work::~n.*").to_string(), "work::~n.*");
        assert_eq!(p("~w.*::~n.*").to_string(), "~w.*::~n.*");
    }
}
