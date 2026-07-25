//! FFI-safe cancellation token handle.
//!
//! Exposes an opaque `*mut CancellationToken` handle that C callers can use to
//! cancel an in-progress extraction.
//!
//! # Ownership Rules
//!
//! - `xberg_cancel_token_new()` allocates and returns an owned handle.
//! - `xberg_cancel_token_free()` takes ownership and deallocates.
//! - All other functions borrow the pointer for the duration of the call.
//! - Null pointers are silently accepted by every function (no-op or false return).
//!
//! # Thread Safety
//!
//! `CancellationToken` is `Send + Sync`, so the handle may be passed between
//! threads freely. Calling `xberg_cancel_token_cancel` from any thread is safe.

use std::os::raw::c_int;

/// Opaque handle to a [`xberg::cancellation::CancellationToken`].
///
/// Allocate with [`xberg_cancel_token_new`].
/// Free with [`xberg_cancel_token_free`].
pub struct CancellationToken {
    pub(crate) inner: xberg::cancellation::CancellationToken,
}

/// Allocate a new, un-cancelled cancellation token.
///
/// # Returns
///
/// Non-null pointer to the token. Must be freed with
/// [`xberg_cancel_token_free`].
///
/// Returns `NULL` only when the allocator fails (which cannot happen in
/// practice on modern platforms without `no-std` overrides).
///
/// # C Signature
///
/// ```c
/// XbergCancellationToken* xberg_cancel_token_new(void);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn xberg_cancel_token_new() -> *mut CancellationToken {
    Box::into_raw(Box::new(CancellationToken {
        inner: xberg::cancellation::CancellationToken::new(),
    }))
}

/// Signal cancellation on the given token.
///
/// All clones of this token (including any held inside an in-progress
/// extraction) will observe [`xberg_cancel_token_is_cancelled`] returning
/// `1` on their next check.
///
/// Passing `NULL` is a no-op.
///
/// # Safety
///
/// `token` must be a valid pointer previously returned by
/// [`xberg_cancel_token_new`] and must not have been freed yet.
///
/// # C Signature
///
/// ```c
/// void xberg_cancel_token_cancel(XbergCancellationToken* token);
/// ```
///
/// # SAFETY
///
/// The pointer is checked for null before dereferencing. If non-null it must
/// point to a valid `CancellationToken` allocated by this crate; the caller
/// guarantees this by only using pointers obtained from
/// `xberg_cancel_token_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_cancel_token_cancel(token: *mut CancellationToken) {
    if token.is_null() {
        return;
    }
    // SAFETY: Caller guarantees `token` is a non-null, live pointer to a
    // `CancellationToken` allocated by `xberg_cancel_token_new`. ~keep
    unsafe { (*token).inner.cancel() };
}

/// Returns `1` if the token has been cancelled, `0` otherwise.
///
/// Passing `NULL` returns `0`.
///
/// # Safety
///
/// Same requirements as [`xberg_cancel_token_cancel`].
///
/// # C Signature
///
/// ```c
/// int xberg_cancel_token_is_cancelled(const XbergCancellationToken* token);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_cancel_token_is_cancelled(token: *const CancellationToken) -> c_int {
    if token.is_null() {
        return 0;
    }
    // SAFETY: Caller guarantees `token` is a non-null, live pointer.
    if unsafe { (*token).inner.is_cancelled() } {
        1
    } else {
        0
    }
}

/// Free a cancellation token handle previously returned by
/// [`xberg_cancel_token_new`].
///
/// Passing `NULL` is a no-op. After this call the pointer must not be used.
///
/// # Safety
///
/// `token` must be either `NULL` or a valid pointer previously returned by
/// [`xberg_cancel_token_new`] that has not yet been freed.
///
/// # C Signature
///
/// ```c
/// void xberg_cancel_token_free(XbergCancellationToken* token);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_cancel_token_free(token: *mut CancellationToken) {
    if token.is_null() {
        return;
    }
    // SAFETY: We checked for null above. The caller guarantees the pointer was
    // returned by `xberg_cancel_token_new` and has not been freed; we take
    // ownership back here and immediately drop the Box. ~keep
    unsafe { drop(Box::from_raw(token)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_is_not_null() {
        let token = xberg_cancel_token_new();
        assert!(!token.is_null());
        unsafe { xberg_cancel_token_free(token) };
    }

    #[test]
    fn test_cancel_sets_flag() {
        let token = xberg_cancel_token_new();
        assert_eq!(unsafe { xberg_cancel_token_is_cancelled(token) }, 0);
        unsafe { xberg_cancel_token_cancel(token) };
        assert_eq!(unsafe { xberg_cancel_token_is_cancelled(token) }, 1);
        unsafe { xberg_cancel_token_free(token) };
    }

    #[test]
    fn test_null_cancel_is_noop() {
        unsafe { xberg_cancel_token_cancel(std::ptr::null_mut()) };
    }

    #[test]
    fn test_null_is_cancelled_returns_zero() {
        assert_eq!(unsafe { xberg_cancel_token_is_cancelled(std::ptr::null()) }, 0);
    }

    #[test]
    fn test_null_free_is_noop() {
        unsafe { xberg_cancel_token_free(std::ptr::null_mut()) };
    }
}
