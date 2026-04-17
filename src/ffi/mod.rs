//! Stable C-ABI surface for external (non-Rust) consumers.
//!
//! The shape mirrors the Rust side: [`ValetRecordIndex`] is the password-free
//! view (label plus uuid) that callers use to render a picker, and
//! [`ValetRecord`] is the materialized form returned by a fetch-by-uuid when
//! the password is actually needed.
//!
//! Strings are UTF-8, length-explicit, heap-allocated by Rust. The caller
//! releases result buffers via [`valet_ffi_record_index_free`] or
//! [`valet_ffi_record_free`]. Any function that returns non-zero sets a
//! thread-local error message retrievable via [`valet_ffi_last_error`].

use crate::{
    Record,
    record::{Label, LabelName},
    uuid::Uuid,
};
use std::{cell::RefCell, ffi::CString, os::raw::c_char, ptr};

#[cfg(all(feature = "db", feature = "ffi"))]
pub mod ops;

/* ---------- Return codes ---------- */

pub const VALET_FFI_OK: i32 = 0;
pub const VALET_FFI_ERR_NULL_ARG: i32 = 1;
pub const VALET_FFI_ERR_INVALID_UTF8: i32 = 2;
pub const VALET_FFI_ERR_IO: i32 = 3;
pub const VALET_FFI_ERR_PROTOCOL: i32 = 4;
pub const VALET_FFI_ERR_PASSWORD_TOO_LONG: i32 = 5;
pub const VALET_FFI_ERR_NOT_FOUND: i32 = 6;

/* ---------- Shared data types ---------- */

/// Heap-allocated UTF-8 byte run.
#[repr(C)]
pub struct ValetStr {
    pub ptr: *mut c_char,
    pub len: usize,
}

impl ValetStr {
    fn from_string(s: String) -> Self {
        Self::from_bytes(s.into_bytes())
    }

    fn from_bytes(mut bytes: Vec<u8>) -> Self {
        bytes.shrink_to_fit();
        let len = bytes.len();
        if len == 0 {
            return ValetStr {
                ptr: ptr::null_mut(),
                len: 0,
            };
        }
        let ptr = bytes.as_mut_ptr() as *mut c_char;
        std::mem::forget(bytes);
        ValetStr { ptr, len }
    }

    /// SAFETY: caller must hold unique ownership.
    unsafe fn free(self) {
        if !self.ptr.is_null() {
            drop(unsafe { Vec::from_raw_parts(self.ptr as *mut u8, self.len, self.len) });
        }
    }
}

/// A single `(key, value)` pair inside a [`ValetRecordIndexEntry::extras`] or
/// [`ValetRecord::extras`] array. Mirrors an entry of Rust's
/// [`Label::extra`](crate::record::Label::extra) map.
#[repr(C)]
pub struct ValetKv {
    pub key: ValetStr,
    pub value: ValetStr,
}

impl ValetKv {
    unsafe fn free(self) {
        unsafe {
            self.key.free();
            self.value.free();
        }
    }
}

fn build_extras(label: &Label) -> (*mut ValetKv, usize) {
    let mut pairs: Vec<ValetKv> = label
        .extra()
        .iter()
        .map(|(k, v)| ValetKv {
            key: ValetStr::from_string(k.clone()),
            value: ValetStr::from_string(v.clone()),
        })
        .collect();
    pairs.shrink_to_fit();
    let count = pairs.len();
    if count == 0 {
        return (ptr::null_mut(), 0);
    }
    let ptr = pairs.as_mut_ptr();
    std::mem::forget(pairs);
    (ptr, count)
}

unsafe fn free_extras(extras: *mut ValetKv, count: usize) {
    if extras.is_null() || count == 0 {
        return;
    }
    let pairs = unsafe { Vec::from_raw_parts(extras, count, count) };
    for kv in pairs {
        unsafe { kv.free() };
    }
}

fn flatten_label(label: &Label) -> String {
    match label.name() {
        LabelName::Simple(s) => s.clone(),
        LabelName::Domain { domain, .. } => domain.clone(),
    }
}

/// Mirror of a [`RecordIndex`](crate::record::RecordIndex) entry: a label
/// plus the uuid of the record it identifies. `extras` is the flat
/// projection of [`Label::extra`](crate::record::Label::extra).
#[repr(C)]
pub struct ValetRecordIndexEntry {
    pub uuid: ValetStr,
    pub label: ValetStr,
    pub extras: *mut ValetKv,
    pub extras_count: usize,
}

impl ValetRecordIndexEntry {
    pub fn new(uuid: &Uuid<Record>, label: &Label) -> Self {
        let (extras, extras_count) = build_extras(label);
        ValetRecordIndexEntry {
            uuid: ValetStr::from_string(uuid.to_string()),
            label: ValetStr::from_string(flatten_label(label)),
            extras,
            extras_count,
        }
    }

    unsafe fn free(self) {
        unsafe {
            self.uuid.free();
            self.label.free();
            free_extras(self.extras, self.extras_count);
        }
    }
}

/// Mirror of Rust's [`RecordIndex`](crate::record::RecordIndex): a flat list
/// of label/uuid pairs with no password material.
#[repr(C)]
pub struct ValetRecordIndex {
    pub entries: *mut ValetRecordIndexEntry,
    pub count: usize,
}

impl ValetRecordIndex {
    pub fn from_entries(entries: &[(Uuid<Record>, Label)]) -> Self {
        let mut views: Vec<ValetRecordIndexEntry> = entries
            .iter()
            .map(|(uuid, label)| ValetRecordIndexEntry::new(uuid, label))
            .collect();
        views.shrink_to_fit();
        let count = views.len();
        if count == 0 {
            return ValetRecordIndex {
                entries: ptr::null_mut(),
                count: 0,
            };
        }
        let entries = views.as_mut_ptr();
        std::mem::forget(views);
        ValetRecordIndex { entries, count }
    }
}

/// Mirror of Rust [`Record`] for the fetch path: same label-side fields as
/// [`ValetRecordIndexEntry`] plus the materialized password.
#[repr(C)]
pub struct ValetRecord {
    pub uuid: ValetStr,
    pub label: ValetStr,
    pub extras: *mut ValetKv,
    pub extras_count: usize,
    pub password: ValetStr,
}

impl ValetRecord {
    pub fn from_record(r: &Record) -> Self {
        let (extras, extras_count) = build_extras(r.label());
        ValetRecord {
            uuid: ValetStr::from_string(r.uuid().to_string()),
            label: ValetStr::from_string(flatten_label(r.label())),
            extras,
            extras_count,
            password: ValetStr::from_bytes(r.password().as_bytes().to_vec()),
        }
    }

    unsafe fn free(self) {
        unsafe {
            self.uuid.free();
            self.label.free();
            free_extras(self.extras, self.extras_count);
            self.password.free();
        }
    }
}

/* ---------- Thread-local last-error ---------- */

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Writes `msg` into the thread-local slot read by
/// [`valet_ffi_last_error`]. Not part of the C ABI: it's a Rust-side helper
/// used by sibling crates (such as `valetd`) that share this module's
/// last-error slot when reporting their own FFI errors.
pub fn set_last_error(msg: impl Into<String>) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(msg.into()).ok();
    });
}

/// Clears the thread-local slot read by [`valet_ffi_last_error`]. Like
/// [`set_last_error`], this is a Rust-side helper rather than part of the
/// C ABI.
pub fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

/// Bridge between a Rust-side error type and the C ABI's integer return
/// codes + [`valet_ffi_last_error`] message slot.
///
/// Implementors live in sibling FFI crates (e.g. `valetd`) so that a single
/// [`report`] helper can funnel all their `Result`s into the FFI surface.
pub trait FfiError {
    /// The `VALET_FFI_ERR_*` code C callers should see on this error.
    fn code(&self) -> i32;
    /// Human-readable message written into the last-error slot.
    fn message(&self) -> String;
}

/// Funnels a `Result<(), E>` into an FFI return code.
///
/// On `Ok(())` the last-error slot is cleared and [`VALET_FFI_OK`] is
/// returned. On `Err(e)` the slot is populated from `e.message()` and
/// `e.code()` is returned. Not part of the C ABI: call this from Rust-side
/// `extern "C"` wrappers.
pub fn report<E: FfiError>(result: Result<(), E>) -> i32 {
    match result {
        Ok(()) => {
            clear_last_error();
            VALET_FFI_OK
        }
        Err(e) => {
            set_last_error(e.message());
            e.code()
        }
    }
}

/* ---------- Exported C functions ---------- */

/// Returns the thread-local last error message set by the most recent failing
/// FFI call, or `NULL` if none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(ptr::null())
    })
}

/// Frees a [`ValetRecordIndex`] previously returned by any Valet FFI entry
/// point. Safe to call with a zero-initialized value.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_record_index_free(index: ValetRecordIndex) {
    if index.entries.is_null() || index.count == 0 {
        return;
    }
    let entries = unsafe { Vec::from_raw_parts(index.entries, index.count, index.count) };
    for e in entries {
        unsafe { e.free() };
    }
}

/// Frees a [`ValetRecord`] previously returned by any Valet FFI entry point.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_record_free(record: ValetRecord) {
    unsafe { record.free() };
}
