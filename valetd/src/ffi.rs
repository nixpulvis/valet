//! IPC client FFI — `valetd_ffi_*`.
//!
//! Exposes an opaque handle ([`ValetdClient`]) that speaks the `valetd` wire
//! protocol. The underlying [`Handler`] is chosen at compile time: the real
//! [`Client`] (a sync-wrapped async Unix-socket round-trip) by default, or
//! the in-process [`Stub`] with `--features stub`. The C surface and its
//! return codes are identical either way.
//!
//! Shared types (`ValetStr`, `ValetRecordIndex`, `ValetRecord`,
//! `VALET_FFI_*` return codes, `valet_ffi_last_error`,
//! `valet_ffi_record_index_free`, `valet_ffi_record_free`) come from
//! `valet::ffi` so clients never link two diverging copies.
//!
//! Each `extern "C"` wrapper builds a [`Request`], drives
//! [`Handler::handle`] to a [`Response`] on a single-thread tokio runtime,
//! and hands a `Result<(), FfiCallError>` to [`valet::ffi::report`], which
//! sets the last-error slot and translates the error into a
//! `VALET_FFI_ERR_*` code.
//!
//! [`Client`]: crate::client::Client
//! [`Stub`]: crate::stub::Stub

#[cfg(not(feature = "stub"))]
use crate::client::Client;
use crate::request::{Error, Request, Response, ResponseError};
use crate::server::Handler;
#[cfg(feature = "stub")]
use crate::stub::Stub;
#[cfg(not(feature = "stub"))]
use std::path::Path;
use std::{ffi::CStr, io, os::raw::c_char, ptr, slice};
use tokio::runtime::Runtime;
use valet::{
    Record,
    ffi::{
        self, FfiError, VALET_FFI_ERR_INVALID_UTF8, VALET_FFI_ERR_IO, VALET_FFI_ERR_NULL_ARG,
        VALET_FFI_ERR_PASSWORD_TOO_LONG, VALET_FFI_ERR_PROTOCOL, ValetRecord, ValetRecordIndex,
        ValetStrList,
    },
    password::Password,
    uuid::Uuid,
};

#[cfg(feature = "stub")]
type Inner = Stub;
#[cfg(not(feature = "stub"))]
type Inner = Client;

/// Opaque FFI handle. The C side only ever sees `*mut ValetdClient`.
///
/// Owns a current-thread tokio runtime used to drive [`Handler::handle`]
/// to completion synchronously — the C ABI is blocking. The inner handler
/// takes `&self` and does its own locking (sockets are guarded by a mutex
/// inside [`Client`]; the stub's state is likewise), so concurrent C
/// callers are serialized correctly without a second mutex here.
///
/// [`Client`]: crate::client::Client
pub struct ValetdClient {
    inner: Inner,
    rt: Runtime,
}

impl ValetdClient {
    fn dispatch(&self, req: Request) -> Result<Response, FfiCallError> {
        Ok(self.rt.block_on(self.inner.handle(req))?)
    }
}

/// Error variants produced inside a `valetd_ffi_*` wrapper. The
/// [`FfiError`] impl maps each to a `VALET_FFI_ERR_*` code and message.
#[derive(Debug)]
enum FfiCallError {
    Null(&'static str),
    InvalidUtf8(&'static str),
    PasswordTooLong,
    Io(io::Error),
    Response(ResponseError),
}

impl From<io::Error> for FfiCallError {
    fn from(e: io::Error) -> Self {
        FfiCallError::Io(e)
    }
}

impl From<Error<io::Error>> for FfiCallError {
    fn from(e: Error<io::Error>) -> Self {
        match e {
            Error::Rpc(e) => FfiCallError::Io(e),
            Error::Response(r) => FfiCallError::Response(r),
        }
    }
}

impl From<ResponseError> for FfiCallError {
    fn from(e: ResponseError) -> Self {
        FfiCallError::Response(e)
    }
}

impl FfiError for FfiCallError {
    fn code(&self) -> i32 {
        match self {
            FfiCallError::Null(_) => VALET_FFI_ERR_NULL_ARG,
            FfiCallError::InvalidUtf8(_) => VALET_FFI_ERR_INVALID_UTF8,
            FfiCallError::PasswordTooLong => VALET_FFI_ERR_PASSWORD_TOO_LONG,
            FfiCallError::Io(_) => VALET_FFI_ERR_IO,
            FfiCallError::Response(_) => VALET_FFI_ERR_PROTOCOL,
        }
    }

    fn message(&self) -> String {
        match self {
            FfiCallError::Null(what) => format!("{what} is null"),
            FfiCallError::InvalidUtf8(what) => format!("{what} is not valid UTF-8"),
            FfiCallError::PasswordTooLong => "password too long".into(),
            FfiCallError::Io(e) => format!("io: {e}"),
            FfiCallError::Response(r) => r.to_string(),
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
/// `p` must be non-null and point to a null-terminated C string valid for
/// reads up to and including the terminator.
unsafe fn cstr_to_string(p: *const c_char, name: &'static str) -> Result<String, FfiCallError> {
    unsafe { CStr::from_ptr(p) }
        .to_str()
        .map(str::to_owned)
        .map_err(|_| FfiCallError::InvalidUtf8(name))
}

/// # Safety
///
/// If `len > 0`, `p` must be non-null and point to `len` initialized bytes.
/// If `len == 0`, `p` may be null.
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

/// Borrow a `ValetdClient` from a raw pointer, null-checked.
unsafe fn borrow<'a>(client: *mut ValetdClient) -> Result<&'a ValetdClient, FfiCallError> {
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

#[cfg(feature = "stub")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_new_stub(out: *mut *mut ValetdClient) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        not_null(out, "out")?;
        let rt = new_runtime()?;
        let boxed = Box::new(ValetdClient {
            inner: Stub::new(),
            rt,
        });
        unsafe { *out = Box::into_raw(boxed) };
        Ok(())
    })())
}

#[cfg(not(feature = "stub"))]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_connect(
    socket_path: *const c_char,
    out: *mut *mut ValetdClient,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        not_null(socket_path, "socket_path")?;
        not_null(out, "out")?;
        let path = unsafe { cstr_to_string(socket_path, "socket_path") }?;
        let rt = new_runtime()?;
        let inner = rt.block_on(Client::connect(Path::new(&path)))?;
        let boxed = Box::new(ValetdClient { inner, rt });
        unsafe { *out = Box::into_raw(boxed) };
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_unlock(
    client: *mut ValetdClient,
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
        client
            .dispatch(Request::Unlock {
                username,
                password: pw,
            })?
            .expect_ok()?;
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_status(
    client: *mut ValetdClient,
    out: *mut ValetStrList,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(out, "out")?;
        let users = client.dispatch(Request::Status)?.expect_users()?;
        unsafe { ptr::write(out, ValetStrList::from_strings(users)) };
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_list_users(
    client: *mut ValetdClient,
    out: *mut ValetStrList,
) -> i32 {
    ffi::report((|| -> Result<(), FfiCallError> {
        let client = unsafe { borrow(client) }?;
        not_null(out, "out")?;
        let users = client.dispatch(Request::ListUsers)?.expect_users()?;
        unsafe { ptr::write(out, ValetStrList::from_strings(users)) };
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_list(
    client: *mut ValetdClient,
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
            .dispatch(Request::List { username, queries })?
            .expect_index()?;
        unsafe { ptr::write(out, ValetRecordIndex::from_entries(&entries)) };
        Ok(())
    })())
}

/// # Safety
///
/// If `count > 0`, `queries` and `query_lens` must each point to `count`
/// initialized elements, and every non-null `queries[i]` must be valid for
/// `query_lens[i]` bytes.
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

/// Domain-suffix search within a single lot. Mirrors
/// [`Request::FindRecords`]: symmetric suffix matching against the record's
/// domain label, no regex. Used by platform code (macOS AutoFill, browser
/// extension) that receives a host string from the OS and wants the same
/// match behavior on both sides.
///
/// TODO: replace with a `Query::Domain` variant on [`Request::List`]
/// that carries suffix semantics across lots; see the TODO on
/// `Request::FindRecords` in `request.rs`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_find_records(
    client: *mut ValetdClient,
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
        let entries = client
            .dispatch(Request::FindRecords {
                username,
                lot,
                query,
            })?
            .expect_index()?;
        unsafe { ptr::write(out, ValetRecordIndex::from_entries(&entries)) };
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_fetch(
    client: *mut ValetdClient,
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
            Uuid::parse(&uuid_str).map_err(|_| FfiCallError::InvalidUtf8("uuid"))?;
        let record = client
            .dispatch(Request::Fetch { username, uuid })?
            .expect_record()?;
        unsafe { ptr::write(out, ValetRecord::from_record(&record)) };
        Ok(())
    })())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valetd_ffi_client_free(client: *mut ValetdClient) {
    if !client.is_null() {
        drop(unsafe { Box::from_raw(client) });
    }
}

#[cfg(all(test, feature = "stub"))]
mod tests {
    use super::*;
    use valet::ffi::{VALET_FFI_OK, valet_ffi_record_free, valet_ffi_record_index_free};

    #[test]
    fn stub_roundtrip_via_ffi() {
        unsafe {
            let mut handle: *mut ValetdClient = ptr::null_mut();
            assert_eq!(valetd_ffi_client_new_stub(&mut handle), VALET_FFI_OK);
            assert!(!handle.is_null());

            let username = std::ffi::CString::new("stub-user").unwrap();

            let mut index = ValetRecordIndex {
                entries: ptr::null_mut(),
                count: 0,
            };
            assert_eq!(
                valetd_ffi_client_list(
                    handle,
                    username.as_ptr(),
                    ptr::null(),
                    ptr::null(),
                    0,
                    &mut index
                ),
                VALET_FFI_OK
            );
            assert_eq!(index.count, 2);

            // Fetch the first entry's record and confirm we get a password.
            let first = index
                .entries
                .as_ref()
                .expect("valetd_ffi_client_list returned an empty/null index");
            let uuid_ptr = first.uuid.ptr;
            let uuid_len = first.uuid.len;
            let mut record = ValetRecord {
                uuid: valet::ffi::ValetStr {
                    ptr: ptr::null_mut(),
                    len: 0,
                },
                label: valet::ffi::ValetStr {
                    ptr: ptr::null_mut(),
                    len: 0,
                },
                username: valet::ffi::ValetStr {
                    ptr: ptr::null_mut(),
                    len: 0,
                },
                extras: ptr::null_mut(),
                extras_count: 0,
                password: valet::ffi::ValetStr {
                    ptr: ptr::null_mut(),
                    len: 0,
                },
            };
            assert_eq!(
                valetd_ffi_client_fetch(handle, username.as_ptr(), uuid_ptr, uuid_len, &mut record),
                VALET_FFI_OK
            );
            assert!(record.password.len > 0);

            valet_ffi_record_free(record);
            valet_ffi_record_index_free(index);
            valetd_ffi_client_free(handle);
        }
    }
}
