//! Thread-local error storage and panic-catching boundary helpers for C FFI exports.

#![allow(unsafe_code)]

use std::{
    any::Any,
    cell::RefCell,
    ffi::{c_char, CStr, CString},
    panic::{catch_unwind, AssertUnwindSafe},
};

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

pub(crate) fn clear_last_error() {
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = CString::default();
    });
}

pub(crate) fn set_last_error(message: impl Into<String>) {
    let raw = message.into();
    let sanitized = if raw.as_bytes().contains(&0) {
        raw.replace('\0', " ")
    } else {
        raw
    };
    let c_string = CString::new(sanitized).unwrap_or_default();
    LAST_ERROR.with(|slot| {
        *slot.borrow_mut() = c_string;
    });
}

pub(crate) fn last_error_ptr() -> *const c_char {
    LAST_ERROR.with(|slot| slot.borrow().as_ptr())
}

fn panic_payload_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(msg) = payload.downcast_ref::<&'static str>() {
        format!("panic across FFI boundary: {msg}")
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        format!("panic across FFI boundary: {msg}")
    } else {
        "panic across FFI boundary".to_owned()
    }
}

/// Runs `f` inside [`catch_unwind`], clearing the thread-local error before running and recording
/// any `Err` or caught panic into the thread-local error slot while returning `default`.
pub(crate) fn ffi_try_value<R>(default: R, f: impl FnOnce() -> Result<R, crate::FfiError>) -> R {
    clear_last_error();
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            set_last_error(error.to_string());
            default
        }
        Err(payload) => {
            set_last_error(panic_payload_message(payload));
            default
        }
    }
}

/// Runs `f` inside [`catch_unwind`], returning `true` on `Ok(())` and `false` on error or panic.
pub(crate) fn ffi_try_bool(f: impl FnOnce() -> Result<(), crate::FfiError>) -> bool {
    ffi_try_value(false, || f().map(|()| true))
}

/// Pointer-returning counterpart to [`ffi_try_bool`], returning `null_mut()` on error or panic.
pub(crate) fn ffi_try_ptr<T>(f: impl FnOnce() -> Result<*mut T, crate::FfiError>) -> *mut T {
    ffi_try_value(std::ptr::null_mut(), f)
}

/// Runs an infallible FFI cleanup closure inside [`catch_unwind`].
pub(crate) fn ffi_infallible<R>(default: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(payload) => {
            set_last_error(panic_payload_message(payload));
            default
        }
    }
}

/// Validates and borrows a required NUL-terminated UTF-8 C string parameter.
///
/// # Safety
///
/// When non-null, `ptr` must point to a valid NUL-terminated byte sequence alive for `'a`.
pub(crate) unsafe fn required_c_str<'a>(
    ptr: *const c_char,
    param_name: &'static str,
) -> Result<&'a str, crate::FfiError> {
    if ptr.is_null() {
        return Err(crate::FfiError::NullPointer(param_name));
    }
    // SAFETY: Caller guarantees `ptr` points to a NUL-terminated C string alive for `'a`.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| crate::FfiError::InvalidUtf8(param_name))
}

/// Validates and borrows an optional NUL-terminated UTF-8 C string parameter (`NULL` -> `None`).
///
/// # Safety
///
/// When non-null, `ptr` must point to a valid NUL-terminated byte sequence alive for `'a`.
pub(crate) unsafe fn optional_c_str<'a>(
    ptr: *const c_char,
    param_name: &'static str,
) -> Result<Option<&'a str>, crate::FfiError> {
    if ptr.is_null() {
        return Ok(None);
    }
    // SAFETY: Caller guarantees non-null `ptr` points to a NUL-terminated C string alive for `'a`.
    let text = unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| crate::FfiError::InvalidUtf8(param_name))?;
    Ok(Some(text))
}
