//! C FFI surface for the macOS AutoFill extension.
//!
//! Strings are UTF-8, length-explicit, heap-allocated by Rust. The caller
//! releases result buffers via the matching `*_free` entry points. Any
//! function that returns non-zero has set a thread-local error message
//! retrievable via [`valet_ipc_last_error`].

use crate::client::{Client, Error};
use std::{
    cell::RefCell,
    ffi::{CStr, CString},
    os::raw::c_char,
    path::Path,
    ptr, slice,
};
use valet::{
    Record,
    password::Password,
    record::{Data, Label},
};

#[cfg(feature = "stub")]
use crate::stub::StubClient;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = RefCell::new(None);
}

fn set_last_error(msg: impl Into<String>) {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::new(msg.into()).ok();
    });
}

fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = None;
    });
}

pub const VALET_IPC_OK: i32 = 0;
pub const VALET_IPC_ERR_NULL_ARG: i32 = 1;
pub const VALET_IPC_ERR_INVALID_UTF8: i32 = 2;
pub const VALET_IPC_ERR_IO: i32 = 3;
pub const VALET_IPC_ERR_PROTOCOL: i32 = 4;
pub const VALET_IPC_ERR_PASSWORD_TOO_LONG: i32 = 5;

// Short aliases for use inside this module.
const OK: i32 = VALET_IPC_OK;
const ERR_NULL_ARG: i32 = VALET_IPC_ERR_NULL_ARG;
const ERR_INVALID_UTF8: i32 = VALET_IPC_ERR_INVALID_UTF8;
const ERR_IO: i32 = VALET_IPC_ERR_IO;
const ERR_PROTOCOL: i32 = VALET_IPC_ERR_PROTOCOL;
const ERR_PASSWORD_TOO_LONG: i32 = VALET_IPC_ERR_PASSWORD_TOO_LONG;

enum Handle {
    Connected(Client),
    #[cfg(feature = "stub")]
    Stub(StubClient),
}

/// Opaque client handle. Rust-side representation is [`Handle`]; Swift only
/// ever sees `*mut CValetClient`.
pub struct CValetClient {
    inner: Handle,
}

#[repr(C)]
pub struct ValetStr {
    pub ptr: *mut c_char,
    pub len: usize,
}

impl ValetStr {
    fn from_string(s: String) -> Self {
        let bytes = s.into_bytes();
        Self::from_bytes(bytes)
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

    /// SAFETY: the caller must hold unique ownership — typically this is
    /// called from the `*_free` routines after the containing struct is
    /// dropped.
    unsafe fn free(self) {
        if !self.ptr.is_null() {
            drop(unsafe { Vec::from_raw_parts(self.ptr as *mut u8, self.len, self.len) });
        }
    }
}

#[repr(C)]
pub struct ValetRecordView {
    pub uuid: ValetStr,
    pub label: ValetStr,
    pub username: ValetStr,
    pub password: ValetStr,
    pub url: ValetStr,
}

impl ValetRecordView {
    fn from_record(r: &Record) -> Self {
        ValetRecordView {
            uuid: ValetStr::from_string(r.uuid().to_string()),
            label: ValetStr::from_string(flatten_label(r.label())),
            username: ValetStr::from_string(extra(r.data(), "username")),
            password: ValetStr::from_bytes(r.password().as_bytes().to_vec()),
            url: ValetStr::from_string(extra(r.data(), "url")),
        }
    }

    unsafe fn free(self) {
        unsafe {
            self.uuid.free();
            self.label.free();
            self.username.free();
            self.password.free();
            self.url.free();
        }
    }
}

fn flatten_label(label: &Label) -> String {
    match label {
        Label::Simple(s) => s.clone(),
        Label::Domain { domain, .. } => domain.clone(),
    }
}

fn extra(data: &Data, key: &str) -> String {
    data.extra().get(key).cloned().unwrap_or_default()
}

#[repr(C)]
pub struct ValetRecordList {
    pub items: *mut ValetRecordView,
    pub count: usize,
}

impl ValetRecordList {
    fn from_records(records: &[Record]) -> Self {
        let mut views: Vec<ValetRecordView> =
            records.iter().map(ValetRecordView::from_record).collect();
        views.shrink_to_fit();
        let count = views.len();
        if count == 0 {
            return ValetRecordList {
                items: ptr::null_mut(),
                count: 0,
            };
        }
        let items = views.as_mut_ptr();
        std::mem::forget(views);
        ValetRecordList { items, count }
    }
}

fn record_error(err: Error) -> i32 {
    match err {
        Error::Io(e) => {
            set_last_error(format!("io: {e}"));
            ERR_IO
        }
        Error::Remote(msg) => {
            set_last_error(format!("remote: {msg}"));
            ERR_PROTOCOL
        }
        Error::UnexpectedResponse => {
            set_last_error("unexpected response variant");
            ERR_PROTOCOL
        }
    }
}

#[cfg(feature = "stub")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ipc_client_new_stub(out: *mut *mut CValetClient) -> i32 {
    if out.is_null() {
        set_last_error("out pointer is null");
        return ERR_NULL_ARG;
    }
    clear_last_error();
    let boxed = Box::new(CValetClient {
        inner: Handle::Stub(StubClient::new()),
    });
    unsafe {
        *out = Box::into_raw(boxed);
    }
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ipc_client_connect(
    socket_path: *const c_char,
    out: *mut *mut CValetClient,
) -> i32 {
    if socket_path.is_null() || out.is_null() {
        set_last_error("null argument");
        return ERR_NULL_ARG;
    }
    let path = match unsafe { CStr::from_ptr(socket_path) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_last_error("socket_path is not valid UTF-8");
            return ERR_INVALID_UTF8;
        }
    };
    clear_last_error();
    match Client::connect(Path::new(path)) {
        Ok(c) => {
            let boxed = Box::new(CValetClient {
                inner: Handle::Connected(c),
            });
            unsafe {
                *out = Box::into_raw(boxed);
            }
            OK
        }
        Err(e) => {
            set_last_error(format!("connect: {e}"));
            ERR_IO
        }
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ipc_client_unlock(
    client: *mut CValetClient,
    username: *const c_char,
    password: *const c_char,
    password_len: usize,
) -> i32 {
    if client.is_null() || username.is_null() || (password.is_null() && password_len > 0) {
        set_last_error("null argument");
        return ERR_NULL_ARG;
    }
    let user = match unsafe { CStr::from_ptr(username) }.to_str() {
        Ok(s) => s,
        Err(_) => {
            set_last_error("username is not valid UTF-8");
            return ERR_INVALID_UTF8;
        }
    };
    let pw_bytes: &[u8] = if password_len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(password as *const u8, password_len) }
    };
    let pw_str = match std::str::from_utf8(pw_bytes) {
        Ok(s) => s,
        Err(_) => {
            set_last_error("password is not valid UTF-8");
            return ERR_INVALID_UTF8;
        }
    };
    let pw: Password = match pw_str.try_into() {
        Ok(p) => p,
        Err(_) => {
            set_last_error("password too long");
            return ERR_PASSWORD_TOO_LONG;
        }
    };
    clear_last_error();
    let handle = unsafe { &mut (*client).inner };
    let result = match handle {
        Handle::Connected(c) => c.unlock(user, pw).map(|_| ()),
        #[cfg(feature = "stub")]
        Handle::Stub(s) => s.unlock(user, pw).map(|_| ()),
    };
    match result {
        Ok(()) => OK,
        Err(e) => record_error(e),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ipc_client_list(
    client: *mut CValetClient,
    service_ids: *const *const c_char,
    service_id_lens: *const usize,
    service_ids_count: usize,
    out: *mut ValetRecordList,
) -> i32 {
    if client.is_null() || out.is_null() {
        set_last_error("null argument");
        return ERR_NULL_ARG;
    }
    let ids: Vec<String> = if service_ids_count == 0 {
        Vec::new()
    } else {
        if service_ids.is_null() || service_id_lens.is_null() {
            set_last_error("null service id array");
            return ERR_NULL_ARG;
        }
        let ptrs = unsafe { slice::from_raw_parts(service_ids, service_ids_count) };
        let lens = unsafe { slice::from_raw_parts(service_id_lens, service_ids_count) };
        let mut out = Vec::with_capacity(service_ids_count);
        for (p, l) in ptrs.iter().zip(lens.iter()) {
            if p.is_null() {
                set_last_error("service id ptr is null");
                return ERR_NULL_ARG;
            }
            let bytes = unsafe { slice::from_raw_parts(*p as *const u8, *l) };
            match std::str::from_utf8(bytes) {
                Ok(s) => out.push(s.to_owned()),
                Err(_) => {
                    set_last_error("service id is not valid UTF-8");
                    return ERR_INVALID_UTF8;
                }
            }
        }
        out
    };
    clear_last_error();
    let handle = unsafe { &mut (*client).inner };
    let result = match handle {
        Handle::Connected(c) => c.list(&ids),
        #[cfg(feature = "stub")]
        Handle::Stub(s) => s.list(&ids),
    };
    match result {
        Ok(records) => {
            unsafe { ptr::write(out, ValetRecordList::from_records(&records)) };
            OK
        }
        Err(e) => record_error(e),
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ipc_client_free(client: *mut CValetClient) {
    if !client.is_null() {
        drop(unsafe { Box::from_raw(client) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ipc_record_list_free(list: ValetRecordList) {
    if list.items.is_null() || list.count == 0 {
        return;
    }
    let views = unsafe { Vec::from_raw_parts(list.items, list.count, list.count) };
    for v in views {
        unsafe { v.free() };
    }
}

/// Returns the thread-local last error message set by the most recent failing
/// FFI call, or `NULL` if none. The pointer is owned by the thread-local and
/// remains valid until the next FFI call on the same thread.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn valet_ipc_last_error() -> *const c_char {
    LAST_ERROR.with(|slot| match slot.borrow().as_ref() {
        Some(cstr) => cstr.as_ptr(),
        None => ptr::null(),
    })
}

#[cfg(all(test, feature = "stub"))]
mod tests {
    use super::*;

    #[test]
    fn stub_roundtrip_via_ffi() {
        unsafe {
            let mut handle: *mut CValetClient = ptr::null_mut();
            assert_eq!(valet_ipc_client_new_stub(&mut handle), OK);
            assert!(!handle.is_null());

            let mut list = ValetRecordList {
                items: ptr::null_mut(),
                count: 0,
            };
            assert_eq!(
                valet_ipc_client_list(handle, ptr::null(), ptr::null(), 0, &mut list),
                OK
            );
            assert_eq!(list.count, 2);

            // Inspect first record.
            let views = slice::from_raw_parts(list.items, list.count);
            let first_label = slice::from_raw_parts(views[0].label.ptr as *const u8, views[0].label.len);
            assert!(first_label == b"ycombinator.com" || first_label == b"example.com");

            valet_ipc_record_list_free(list);
            valet_ipc_client_free(handle);
        }
    }
}
