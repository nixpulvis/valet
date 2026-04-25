//! Minimal parser/serialiser for git's on-disk `config` file.
//!
//! Lossless: reading a config and writing it back without mutation
//! reproduces the original bytes (modulo trailing newlines). Only the
//! sections we mutate are rewritten.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::Error;
use crate::remote::Remote;

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

    /// All `[remote "<name>"]` sections in `git_dir/config` with a
    /// `url = ...` entry, as [`Remote`]s. Sections without a URL are
    /// skipped. A missing config file yields an empty list.
    pub(crate) fn list_remotes(git_dir: &Path) -> Result<Vec<Remote>, Error> {
        Ok(Self::read(git_dir)?.parse_remotes().collect())
    }

    /// Look up a remote by name. Errors if no such remote is configured.
    pub(crate) fn lookup_remote(git_dir: &Path, name: &str) -> Result<Remote, Error> {
        Self::read(git_dir)?
            .parse_remotes()
            .find(|r| r.name == name)
            .ok_or_else(|| Error::Other(format!("remote {name:?} not found")))
    }

    /// Add a new `[remote "<name>"]` section pointing at `url`. Errors
    /// if `name` is invalid or already configured.
    pub(crate) fn add_remote(git_dir: &Path, name: &str, url: &str) -> Result<(), Error> {
        validate_remote_name(name)?;
        let mut config = Self::read(git_dir)?;
        if config.has_remote_section(name) {
            return Err(Error::Other(format!("remote {name:?} already exists")));
        }
        config.append_remote_section(name, url);
        config.write(git_dir)
    }

    /// Drop the `[remote "<name>"]` section. Errors if no such remote
    /// is configured.
    pub(crate) fn remove_remote(git_dir: &Path, name: &str) -> Result<(), Error> {
        let mut config = Self::read(git_dir)?;
        if config.drop_remote_sections(name) == 0 {
            return Err(Error::Other(format!("remote {name:?} not found")));
        }
        config.write(git_dir)
    }

    fn parse_remotes(&self) -> impl Iterator<Item = Remote> + '_ {
        self.sections.iter().filter_map(|(header, body)| {
            let name = parse_remote_header(header)?;
            let url = body
                .iter()
                .filter_map(|l| parse_body_kv(l))
                .find(|(k, _)| *k == "url")
                .map(|(_, v)| v.to_string())?;
            Some(Remote {
                name: name.to_string(),
                url,
            })
        })
    }

    fn has_remote_section(&self, name: &str) -> bool {
        self.sections
            .iter()
            .any(|(h, _)| parse_remote_header(h) == Some(name))
    }

    fn append_remote_section(&mut self, name: &str, url: &str) {
        self.sections.push((
            format!("[remote \"{name}\"]"),
            vec![
                format!("\turl = {url}"),
                format!("\tfetch = +refs/heads/*:refs/remotes/{name}/*"),
            ],
        ));
    }

    fn drop_remote_sections(&mut self, name: &str) -> usize {
        let before = self.sections.len();
        self.sections
            .retain(|(h, _)| parse_remote_header(h) != Some(name));
        before - self.sections.len()
    }
}

fn validate_remote_name(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::Other("remote name is empty".into()));
    }
    if name
        .chars()
        .any(|c| c == '"' || c == '\n' || c == '\r' || c == '[' || c == ']')
    {
        return Err(Error::Other(format!(
            "remote name {name:?} contains invalid characters"
        )));
    }
    Ok(())
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
