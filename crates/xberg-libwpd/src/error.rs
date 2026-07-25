use thiserror::Error;

/// Errors returned when extracting a WordPerfect document.
#[derive(Error, Debug)]
pub enum WpdError {
    /// The input buffer was empty or a null pointer reached the shim.
    #[error("invalid arguments passed to libwpd shim")]
    InvalidArgs,

    /// The buffer is not a WordPerfect document libwpd recognizes.
    #[error("not a supported WordPerfect document")]
    UnsupportedFormat,

    /// libwpd recognized the format but failed to parse the document.
    #[error("libwpd failed to parse the document")]
    ParseError,

    /// The shim could not allocate the output buffer.
    #[error("out of memory while extracting text")]
    OutOfMemory,

    /// A C++ exception was caught at the FFI boundary.
    #[error("libwpd raised an unexpected error")]
    Internal,

    /// The extracted text was not valid UTF-8.
    #[error("libwpd returned invalid UTF-8")]
    InvalidUtf8,

    /// WordPerfect extraction is not available on this platform.
    #[error("WordPerfect extraction is not supported on this platform")]
    UnsupportedPlatform,
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl WpdError {
    /// Map a shim result code (see `shim.cpp`) to an error. Code 0 is success
    /// and has no error representation.
    pub(crate) fn from_code(code: i32) -> Self {
        match code {
            1 => WpdError::InvalidArgs,
            2 => WpdError::UnsupportedFormat,
            3 => WpdError::ParseError,
            4 => WpdError::OutOfMemory,
            _ => WpdError::Internal,
        }
    }
}
