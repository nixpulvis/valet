//! Labeled, versioned entry storage backed by git.
//!
//! A storgit [`Store`] creates a git repository to use as a database for
//! labeled data. The store provides a thin wrapper around the [`Layout`]
//! interface, as well as layout specific constructors and storage methods.
//!
//! Each entry is keyed by an opaque [`EntryId`] and carries two optional payloads
//! inside every commit: a [`label`][label] (cached metadata the caller wants
//! to scan without reading from git) and a [`data`][data] blob (the actual
//! record). The store's [`put`][put] method writes a commit and updates the
//! entry's ref, [`get`][get] returns the latest [`Entry`], and
//! [`history`][history] walks the entry's commits. More information can be
//! found on the [`Layout`] trait, or each implementation's docs.
//!
//! [put]: Store::put
//! [get]: Store::get
//! [history]: Store::history
//! [label]: Entry::label
//! [data]: Entry::data
//!
//! storgit is payload-agnostic: it stores raw bytes. Encryption, id policy,
//! label format, and any higher-level semantics belong to the caller.
//!
//! [`Layout`]: layout::Layout

mod entry;
mod error;
mod git;
pub mod id;
pub mod layout;
mod module;
mod parent;
mod store;
mod tarball;

pub use entry::Entry;
pub use error::Error;
pub use id::{CommitId, EntryId};
pub use layout::subdir::SubdirLayout;
pub use layout::submodule::SubmoduleLayout;
pub use store::Store;
