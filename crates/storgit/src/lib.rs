//! Identified, versioned entry storage backed by git.
//!
//! A [`Store`] is a thin wrapper around a [`layout::Layout`] that
//! owns a scratch [`tempfile::TempDir`]. The default layout is
//! [`layout::submodule::SubmoduleLayout`], which persists as a parent
//! bare repo whose tree carries one gitlink per live entry plus a
//! `.gitmodules` manifest, and one bare submodule repo per entry id
//! with its own object database. See that module for the full shape.
//!
//! Each entry is keyed by an opaque [`Id`] and carries two optional
//! payloads inside every commit: a `label` (searchable metadata the
//! caller wants to scan without opening modules) and a `data` blob
//! (the actual record). `put` writes a commit and updates the entry's
//! ref; `get` returns the latest [`Entry`]; `history` walks the
//! entry's commits.
//!
//! storgit is payload-agnostic: it stores raw bytes. Encryption,
//! id policy, label format, and any higher-level semantics belong to
//! the caller.

mod entry;
mod error;
mod git;
pub mod id;
pub mod layout;
mod module;
mod parent;
mod store;
mod tarball;

pub use entry::{CommitId, Entry};
pub use error::Error;
pub use id::Id;
pub use layout::submodule::{ModuleChange, ModuleFetcher, Modules, Parts, Snapshot};
pub use store::Store;
