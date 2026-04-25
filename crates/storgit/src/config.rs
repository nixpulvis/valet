//! Minimal parser/serialiser for git's on-disk `config` file.
//!
//! Lossless: reading a config and writing it back without mutation
//! reproduces the original bytes (modulo trailing newlines). Only the
//! sections we mutate are rewritten.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Error;

/// A single `[remote "<name>"]` entry extracted from a [`GitConfig`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub url: String,
}

/// Parsed view of a git `config` file, as a sequence of logical
/// sections. Each section is `(header_line, body_lines)`; blank
/// lines and comments before the first header attach to a leading
/// synthetic section with an empty header.
pub(crate) struct GitConfig {
    sections: Vec<(String, Vec<String>)>,
}

impl GitConfig {
    /// Read and parse `git_dir/config`. A missing file is an empty
    /// config (only the synthetic leading section), not an error,
    /// because storgit tolerates repos without a config.
    pub(crate) fn read(git_dir: &Path) -> Result<Self, Error> {
        let text = match fs::read_to_string(Self::path(git_dir)) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self::parse(&text))
    }

    /// Serialise and write back to `git_dir/config`.
    pub(crate) fn write(&self, git_dir: &Path) -> Result<(), Error> {
        fs::write(Self::path(git_dir), self.serialize())?;
        Ok(())
    }

    pub(crate) fn path(git_dir: &Path) -> PathBuf {
        git_dir.join("config")
    }

    fn parse(text: &str) -> Self {
        let mut sections: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];
        for line in text.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with('[') {
                sections.push((line.to_string(), Vec::new()));
            } else {
                sections.last_mut().unwrap().1.push(line.to_string());
            }
        }
        Self { sections }
    }

    fn serialize(&self) -> String {
        let mut s = String::new();
        for (header, body) in &self.sections {
            if !header.is_empty() {
                s.push_str(header);
                s.push('\n');
            }
            for line in body {
                s.push_str(line);
                s.push('\n');
            }
        }
        s
    }

    /// Iterate every `[remote "<name>"]` section with a `url = ...`
    /// entry, yielding a [`RemoteEntry`] per match. Sections without
    /// a URL are skipped.
    pub(crate) fn remotes(&self) -> impl Iterator<Item = RemoteEntry> + '_ {
        self.sections.iter().filter_map(|(header, body)| {
            let name = parse_remote_header(header)?;
            let url = body
                .iter()
                .filter_map(|l| parse_body_kv(l))
                .find(|(k, _)| *k == "url")
                .map(|(_, v)| v.to_string())?;
            Some(RemoteEntry {
                name: name.to_string(),
                url,
            })
        })
    }

    pub(crate) fn has_remote(&self, name: &str) -> bool {
        self.sections
            .iter()
            .any(|(h, _)| parse_remote_header(h) == Some(name))
    }

    /// Append a new `[remote "<name>"]` section. Does not check for
    /// duplicates; callers filter via [`Self::has_remote`] first.
    pub(crate) fn add_remote(&mut self, name: &str, url: &str) {
        self.sections.push((
            format!("[remote \"{name}\"]"),
            vec![
                format!("\turl = {url}"),
                format!("\tfetch = +refs/heads/*:refs/remotes/{name}/*"),
            ],
        ));
    }

    /// Drop every `[remote "<name>"]` section matching `name`.
    /// Returns how many were removed so callers can error on 0.
    pub(crate) fn remove_remote(&mut self, name: &str) -> usize {
        let before = self.sections.len();
        self.sections
            .retain(|(h, _)| parse_remote_header(h) != Some(name));
        before - self.sections.len()
    }
}

fn parse_remote_header(header: &str) -> Option<&str> {
    let h = header.trim();
    let rest = h.strip_prefix('[')?.strip_suffix(']')?.trim();
    let rest = rest.strip_prefix("remote")?.trim_start();
    let inner = rest.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner)
}

fn parse_body_kv(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    if t.starts_with('#') || t.starts_with(';') || t.is_empty() {
        return None;
    }
    let eq = t.find('=')?;
    let key = t[..eq].trim();
    let val = t[eq + 1..].trim();
    Some((key, val))
}
