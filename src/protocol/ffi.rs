//! C ABI surface over the typed [`Handler::call`] API.
//!
//! Structure:
//!
//! * [`ValetClient`] - opaque handle holding a concrete [`Handler`]
//!   plus a tokio runtime to block on its async methods. The handler
//!   type is feature-selected at compile time ([`EmbeddedHandler`]
//!   with `protocol-embedded`, otherwise [`SocketClient`]); exactly
//!   one constructor is visible per build.
//! * Per-protocol constructors: `valet_ffi_client_new_embedded`,
//!   `valet_ffi_client_new_socket`. Each is gated on its protocol
//!   feature; with both enabled, `protocol-embedded` wins by
//!   precedence.
//! * Per-method shims: `valet_ffi_client_status`, `_unlock`, `_list`,
//!   `_find_records`, `_fetch`, `_list_users`, `_free`. These don't
//!   care which handler the handle wraps; they issue typed [`Call`]s
//!   through the shared [`Handler::call`] surface.
//!
//! Shared types (`ValetStr`, `ValetRecordIndex`, `ValetRecord`,
//! `VALET_FFI_*` return codes, `valet_ffi_last_error`,
//! `valet_ffi_record_index_free`, `valet_ffi_record_free`) come from
//! [`crate::ffi`] so clients never link two diverging copies.
//!
//! [`Handler`]: super::Handler
//! [`Handler::call`]: super::Handler::call
//! [`Call`]: crate::protocol::message::Call
//! [`EmbeddedHandler`]: crate::protocol::EmbeddedHandler
//! [`SocketClient`]: crate::protocol::SocketClient

use super::{Error as ClientError, Handler};
use crate::ffi::{
    self, FfiError, VALET_FFI_ERR_INVALID_ARG, VALET_FFI_ERR_INVALID_UTF8, VALET_FFI_ERR_IO,
    VALET_FFI_ERR_NULL_ARG, VALET_FFI_ERR_PASSWORD_TOO_LONG, VALET_FFI_ERR_PROTOCOL, ValetRecord,
    ValetRecordIndex, ValetStrList,
};
#[cfg(feature = "protocol-embedded")]
use crate::protocol::EmbeddedHandler;
#[cfg(all(feature = "protocol-socket", not(feature = "protocol-embedded")))]
use crate::protocol::SocketClient;
use crate::protocol::message::{FindRecords, List, ListUsers, Status, Unlock};
use crate::{Record, password::Password, uuid::Uuid};
#[cfg(all(feature = "protocol-socket", not(feature = "protocol-embedded")))]
use std::path::Path;
use std::{ffi::CStr, io, os::raw::c_char, ptr, slice};
use tokio::runtime::Runtime;

/// The concrete handler the linked staticlib wraps. Feature-selected
/// at compile time: with `protocol-embedded` the handle carries an
/// in-proc local handler; otherwise with `protocol-socket` it
/// carries a socket client.
#[cfg(feature = "protocol-embedded")]
type Inner = EmbeddedHandler;
#[cfg(all(feature = "protocol-socket", not(feature = "protocol-embedded")))]
type Inner = SocketClient;

/// Opaque FFI handle. The C side only ever sees `*mut ValetClient`.
///
/// Owns a multi-threaded tokio runtime used to drive the async
/// handler methods to completion synchronously - the C ABI is
/// blocking. The inner handler takes `&self` and does its own locking,
/// so concurrent C callers are serialized correctly without a second
/// mutex here.
pub struct ValetClient {
    inner: Inner,
    rt: Runtime,
}

/// Error variants produced inside a `valet_ffi_client_*` wrapper.
#[derive(Debug)]
enum FfiCallError {
    Null(&'static str),
    InvalidUtf8(&'static str),
    InvalidUuid(&'static str),
    PasswordTooLong,
    Io(io::Error),
    Remote(String),
    Unexpected,
}

impl From<io::Error> for FfiCallError {
    fn from(e: io::Error) -> Self {
        FfiCallError::Io(e)
    }
}

impl From<ClientError> for FfiCallError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::Io(e) => FfiCallError::Io(e),
            ClientError::Remote(msg) => FfiCallError::Remote(msg),
            ClientError::Unexpected => FfiCallError::Unexpected,
        }
    }
}

impl FfiError for FfiCallError {
    fn code(&self) -> i32 {
        match self {
            FfiCallError::Null(_) => VALET_FFI_ERR_NULL_ARG,
            FfiCallError::InvalidUtf8(_) => VALET_FFI_ERR_INVALID_UTF8,
            FfiCallError::InvalidUuid(_) => VALET_FFI_ERR_INVALID_ARG,
            FfiCallError::PasswordTooLong => VALET_FFI_ERR_PASSWORD_TOO_LONG,
            FfiCallError::Io(_) => VALET_FFI_ERR_IO,
            FfiCallError::Remote(_) | FfiCallError::Unexpected => VALET_FFI_ERR_PROTOCOL,
        }
    }

    fn message(&self) -> String {
        match self {
            FfiCallError::Null(what) => format!("{what} is null"),
            FfiCallError::InvalidUtf8(what) => format!("{what} is not valid UTF-8"),
            FfiCallError::InvalidUuid(what) => format!("{what} is not a valid UUID"),
            FfiCallError::PasswordTooLong => "password too long".into(),
            FfiCallError::Io(e) => format!("io: {e}"),
            FfiCallError::Remote(msg) => format!("remote: {msg}"),
            FfiCallError::Unexpected => "unexpected response variant".into(),
        }
    }
}

fn not_null<T>(p: *const T, name: &'static str) -> Result<(), FfiCallError> {
    if p.is_null() {
        Err(FfiCallError::Null(name))
    } else {
        Ok(())
    }
}

/// # Safety
///
/// `p` must be non-null and point to a null-terminated C string valid
/// for reads up to and including the terminator.
unsafe fn cstr_to_string(p: *const c_char, name: &'static str) -> Result<String, FfiCallError> {
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| FfiCallError::InvalidUtf8(name))
}

/// # Safety
///
/// If `len > 0`, `p` must be non-null and point to `len` initialized
/// bytes. If `len == 0`, `p` may be null.
unsafe fn bytes_to_string(
    p: *const c_char,
    len: usize,
    name: &'static str,
) -> Result<String, FfiCallError> {
    let bytes = if len == 0 {
        &[][..]
    } else {
        unsafe { slice::from_raw_parts(p as *const u8, len) }
    };
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| FfiCallError::InvalidUtf8(name))
}

/// Borrow a `ValetClient` from a raw pointer, null-checked.
unsafe fn borrow<'a>(client: *mut ValetClient) -> Result<&'a ValetClient, FfiCallError> {
    unsafe { client.as_ref() }.ok_or(FfiCallError::Null("client"))
}

fn new_runtime() -> io::Result<Runtime> {
    // Multi-thread so callers can invoke valet's storgit work, which
    // uses `tokio::task::block_in_place` internally for the DB-backed
    // fetcher. `block_in_place` panics on a `current_thread` runtime.
    // A small worker count keeps the cost low for the FFI use case
    // (one in-process call at a time).
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
}

/// # Safety
///
/// `db_path` must point to a null-terminated UTF-8 C string. `out`
/// must be non-null and writable. On success `*out` holds a freshly
/// boxed [`ValetClient`] that the caller frees with
/// [`valet_ffi_client_free`].
#[cfg(feature = "protocol-embedded")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_new_embedded(
    db_path: *const c_char,
    out: *mut *mut ValetClient,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        not_null(db_path, "db_path")?;
        not_null(out, "out")?;
        let path = unsafe { cstr_to_string(db_path, "db_path") }?;
        let rt = new_runtime()?;
        let db = rt
            .block_on(crate::db::Database::new(&path))
            .map_err(|e| FfiCallError::Io(io::Error::other(format!("{e:?}"))))?;
        let inner = EmbeddedHandler::new(db, rt.handle());
        let boxed = Box::new(ValetClient { inner, rt });
        unsafe { *out = Box::into_raw(boxed) };
        Ok(())
    })())
}

/// # Safety
///
/// `socket_path` must point to a null-terminated UTF-8 C string. `out`
/// must be non-null and writable. On success `*out` holds a freshly
/// boxed [`ValetClient`] that the caller frees with
/// [`valet_ffi_client_free`].
#[cfg(all(feature = "protocol-socket", not(feature = "protocol-embedded")))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_new_socket(
    socket_path: *const c_char,
    out: *mut *mut ValetClient,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        not_null(socket_path, "socket_path")?;
        not_null(out, "out")?;
        let path = unsafe { cstr_to_string(socket_path, "socket_path") }?;
        let rt = new_runtime()?;
        let inner = rt.block_on(SocketClient::connect(Path::new(&path)))?;
        let boxed = Box::new(ValetClient { inner, rt });
        unsafe { *out = Box::into_raw(boxed) };
        Ok(())
    })())
}

/// # Safety
///
/// `client` must be a valid pointer previously returned by one of the
/// `valet_ffi_client_new_*` constructors and not yet freed. `username`
/// must point to a null-terminated UTF-8 C string. If `password_len > 0`,
/// `password` must point to `password_len` initialized bytes; if
/// `password_len == 0`, `password` may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_unlock(
    client: *mut ValetClient,
    username: *const c_char,
    password: *const c_char,
    password_len: usize,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(username, "username")?;
        if password.is_null() && password_len > 0 {
            return Err(FfiCallError::Null("password"));
        }
        let username = unsafe { cstr_to_string(username, "username") }?;
        let pw_str = unsafe { bytes_to_string(password, password_len, "password") }?;
        let pw: Password = pw_str
            .as_str()
            .try_into()
            .map_err(|_| FfiCallError::PasswordTooLong)?;
        client.rt.block_on(client.inner.call(Unlock {
            username,
            password: pw,
        }))?;
        Ok(())
    })())
}

/// # Safety
///
/// `client` must be a valid pointer previously returned by one of the
/// `valet_ffi_client_new_*` constructors and not yet freed. `out` must
/// be non-null and writable; on success it receives a heap-allocated
/// [`ValetStrList`] that the caller must release with
/// [`crate::ffi::valet_ffi_str_list_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_status(
    client: *mut ValetClient,
    out: *mut ValetStrList,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(out, "out")?;
        let users = client.rt.block_on(client.inner.call(Status))?;
        unsafe { ptr::write(out, ValetStrList::from_strings(users)) };
        Ok(())
    })())
}

/// # Safety
///
/// Same rules as [`valet_ffi_client_status`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_list_users(
    client: *mut ValetClient,
    out: *mut ValetStrList,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(out, "out")?;
        let users = client.rt.block_on(client.inner.call(ListUsers))?;
        unsafe { ptr::write(out, ValetStrList::from_strings(users)) };
        Ok(())
    })())
}

/// # Safety
///
/// `client` must be a valid pointer previously returned by one of the
/// `valet_ffi_client_new_*` constructors and not yet freed.
/// `username` must point to a null-terminated UTF-8 C string. If
/// `queries_count > 0`, `queries` and `query_lens` must each point to
/// `queries_count` initialized elements, every non-null `queries[i]`
/// valid for `query_lens[i]` bytes. `out` must be non-null and
/// writable; on success it receives a heap-allocated
/// [`ValetRecordIndex`] that the caller must release with
/// [`crate::ffi::valet_ffi_record_index_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_list(
    client: *mut ValetClient,
    username: *const c_char,
    queries: *const *const c_char,
    query_lens: *const usize,
    queries_count: usize,
    out: *mut ValetRecordIndex,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(username, "username")?;
        not_null(out, "out")?;
        let username = unsafe { cstr_to_string(username, "username") }?;
        let queries = unsafe { collect_queries(queries, query_lens, queries_count) }?;
        let entries = client
            .rt
            .block_on(client.inner.call(List { username, queries }))?;
        unsafe { ptr::write(out, ValetRecordIndex::from_entries(&entries)) };
        Ok(())
    })())
}

/// # Safety
///
/// If `count > 0`, `queries` and `query_lens` must each point to
/// `count` initialized elements, and every non-null `queries[i]` must
/// be valid for `query_lens[i]` bytes.
unsafe fn collect_queries(
    queries: *const *const c_char,
    query_lens: *const usize,
    count: usize,
) -> Result<Vec<String>, FfiCallError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    not_null(queries, "queries")?;
    not_null(query_lens, "query_lens")?;
    let ptrs = unsafe { slice::from_raw_parts(queries, count) };
    let lens = unsafe { slice::from_raw_parts(query_lens, count) };
    let mut out = Vec::with_capacity(count);
    for (p, l) in ptrs.iter().zip(lens.iter()) {
        not_null(*p, "queries[i]")?;
        out.push(unsafe { bytes_to_string(*p, *l, "queries[i]") }?);
    }
    Ok(out)
}

/// # Safety
///
/// `client` must be a valid pointer previously returned by one of the
/// `valet_ffi_client_new_*` constructors and not yet freed. `username`
/// and `lot` must point to null-terminated UTF-8 C strings. If
/// `domain_len > 0`, `domain` must point to `domain_len` initialized
/// bytes; if `domain_len == 0`, `domain` may be null. `out` must be
/// non-null and writable; on success it receives a heap-allocated
/// [`ValetRecordIndex`] that the caller must release with
/// [`crate::ffi::valet_ffi_record_index_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_find_records(
    client: *mut ValetClient,
    username: *const c_char,
    lot: *const c_char,
    domain: *const c_char,
    domain_len: usize,
    out: *mut ValetRecordIndex,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(username, "username")?;
        not_null(lot, "lot")?;
        not_null(out, "out")?;
        let username = unsafe { cstr_to_string(username, "username") }?;
        let lot = unsafe { cstr_to_string(lot, "lot") }?;
        let query = unsafe { bytes_to_string(domain, domain_len, "domain") }?;
        let entries = client.rt.block_on(client.inner.call(FindRecords {
            username,
            lot,
            query,
        }))?;
        unsafe { ptr::write(out, ValetRecordIndex::from_entries(&entries)) };
        Ok(())
    })())
}

/// # Safety
///
/// `client` must be a valid pointer previously returned by one of the
/// `valet_ffi_client_new_*` constructors and not yet freed. `username`
/// must point to a null-terminated UTF-8 C string. If `uuid_len > 0`,
/// `uuid` must point to `uuid_len` initialized bytes containing the
/// UTF-8 string representation of the UUID. `out` must be non-null
/// and writable; on success it receives a heap-allocated
/// [`ValetRecord`] that the caller must release with
/// [`crate::ffi::valet_ffi_record_free`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_fetch(
    client: *mut ValetClient,
    username: *const c_char,
    uuid: *const c_char,
    uuid_len: usize,
    out: *mut ValetRecord,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(username, "username")?;
        not_null(uuid, "uuid")?;
        not_null(out, "out")?;
        let username = unsafe { cstr_to_string(username, "username") }?;
        let uuid_str = unsafe { bytes_to_string(uuid, uuid_len, "uuid") }?;
        let uuid: Uuid<Record> =
            Uuid::parse(&uuid_str).map_err(|_| FfiCallError::InvalidUuid("uuid"))?;
        let record = client.rt.block_on(
            client
                .inner
                .call(crate::protocol::message::Fetch { username, uuid }),
        )?;
        unsafe { ptr::write(out, ValetRecord::from_record(&record)) };
        Ok(())
    })())
}

/// # Safety
///
/// `client` must be either null or a pointer previously returned by
/// one of the `valet_ffi_client_new_*` constructors and not yet
/// freed. After this call the pointer must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ffi_client_free(client: *mut ValetClient) {
    if !client.is_null() {
        drop(unsafe { Box::from_raw(client) });
    }
}
