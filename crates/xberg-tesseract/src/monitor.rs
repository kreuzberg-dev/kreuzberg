use crate::error::{Result, TesseractError};
use std::os::raw::{c_int, c_void};
use std::sync::{Arc, Mutex};

pub struct TessMonitor {
    handle: Arc<Mutex<*mut c_void>>,
}

unsafe impl Send for TessMonitor {}
unsafe impl Sync for TessMonitor {}

impl TessMonitor {
    /// Creates a new instance of the TessMonitor.
    ///
    /// # Returns
    ///
    /// Returns the new instance of the TessMonitor.
    pub fn new() -> Self {
        let handle = unsafe { TessMonitorCreate() };
        TessMonitor {
            handle: Arc::new(Mutex::new(handle)),
        }
    }

    /// Sets the deadline for the monitor.
    ///
    /// # Arguments
    ///
    /// * `deadline` - Deadline in milliseconds.
    ///
    /// # Errors
    ///
    /// Returns a `TesseractError::MutexLockError` if the mutex lock fails.
    pub fn set_deadline(&self, deadline: i32) -> Result<()> {
        let handle = self.handle.lock().map_err(|_| TesseractError::MutexLockError)?;
        unsafe { TessMonitorSetDeadlineMSecs(*handle, deadline) };
        Ok(())
    }

    /// Gets the progress of the monitor.
    ///
    /// # Returns
    ///
    /// Returns the progress as an `i32` if successful, otherwise returns an error.
    ///
    /// # Errors
    ///
    /// Returns a `TesseractError::MutexLockError` if the mutex lock fails.
    pub fn get_progress(&self) -> Result<i32> {
        let handle = self.handle.lock().map_err(|_| TesseractError::MutexLockError)?;
        Ok(unsafe { TessMonitorGetProgress(*handle) })
    }

    /// Returns the raw underlying monitor handle.
    ///
    /// Intended for internal use by [`crate::TesseractAPI::recognize_with_monitor`]
    /// to pass the monitor into `TessBaseAPIRecognize`.
    ///
    /// # Errors
    ///
    /// Returns a `TesseractError::MutexLockError` if the mutex lock fails.
    pub(crate) fn as_ptr(&self) -> Result<*mut c_void> {
        let handle = self.handle.lock().map_err(|_| TesseractError::MutexLockError)?;
        Ok(*handle)
    }
}

impl Drop for TessMonitor {
    fn drop(&mut self) {
        if let Ok(handle) = self.handle.lock() {
            unsafe { TessMonitorDelete(*handle) };
        }
    }
}

ffi_extern! {
    pub fn TessMonitorCreate() -> *mut c_void;
    pub fn TessMonitorDelete(monitor: *mut c_void);
    pub fn TessMonitorSetDeadlineMSecs(monitor: *mut c_void, deadline: c_int);
    pub fn TessMonitorGetProgress(monitor: *mut c_void) -> c_int;
}
