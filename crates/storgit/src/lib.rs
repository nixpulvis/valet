//! Identified, versioned entry storage backed by git.
//!
//! A [`Store`] is two things living under a scratch
//! [`tempfile::TempDir`]:
//!
//! - A full parent bare repo (`parent.git/`) whose tree carries one
//!   gitlink per live entry, a `.gitmodules` manifest, and an
//!   `index/` subtree of label blobs. Holds its own objects.
//! - One full bare submodule per entry id (`modules/<id>.git/`), each
//!   with its own object database; `refs/heads/main` records that
//!   entry's latest commit.
//!
//! There is no shared object database: each module owns its own
//! objects so that a fresh [`Store::open`] can ignore them entirely
//! and the `index/` cache in the parent gives a working label index
//! without touching any submodule. Modules reach the store through
//! one of three paths, all converging on the same on-disk scratch:
//! [`Parts::modules`] (handed over at open time), a
//! [`Parts::fetcher`] consulted on demand for misses, or explicit
//! [`Store::load_module`] pushes after open.
//!
//! Each entry is keyed by an opaque `id` and carries two optional
//! payloads inside every commit: a `label` (searchable metadata the
//! caller wants to scan without opening modules) and a `data` blob
//! (the actual record). `put` writes a commit to that module's
//! objects DB and updates the entry's ref; `get` returns the latest
//! [`Entry`]; `history` walks the entry's commits.
//!
//! Persistence is split per-row. Callers load through [`Parts`]
//! ([`Parts::parent`], [`Parts::modules`]) and persist only what
//! [`Snapshot`] flags as changed, so writing one entry rewrites that
//! one entry's tarball plus the parent's, not every other entry's.
//!
//! storgit is payload-agnostic: it stores raw bytes. Encryption,
//! id policy, label format, and any higher-level semantics belong to
//! the caller.

mod entry;
mod error;
mod git;
pub mod id;
mod module;
mod parent;
mod persist;
mod store;
mod tarball;

pub use entry::{CommitId, Entry};
pub use error::Error;
pub use id::Id;
pub use persist::{ModuleChange, ModuleFetcher, Modules, Parts, Snapshot};
pub use store::Store;
