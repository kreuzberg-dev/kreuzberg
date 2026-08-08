//! pdf_oxide backend for PDF extraction.
//!
//! Provides text extraction,
//! metadata parsing, annotation extraction, image extraction, table detection,
//! form field extraction, and font metrics for heading hierarchy detection.

pub(crate) mod annotations;
pub(crate) mod forms;
pub(crate) mod hierarchy;
pub(crate) mod images;
pub(crate) mod metadata;
pub(crate) mod span_geometry;
pub(crate) mod table;
pub(crate) mod text;

use crate::Result;
use crate::error::XbergError;

/// Run a synchronous `pdf_oxide` operation under [`std::panic::catch_unwind`],
/// converting a panic into an ordinary error produced by `on_panic`.
///
/// `pdf_oxide`'s reading-order span sort — and the tategaki table strategy —
/// can panic with *"user-provided comparison function does not correctly
/// implement a total order"* on PDFs whose glyph geometry yields NaN or
/// non-antisymmetric comparison keys (xberg #1198). Because these calls run
/// synchronously on a Tokio worker, an uncaught panic unwinds through the async
/// boundary and surfaces to language bindings as an opaque `RustPanic`, aborting
/// the whole extraction. Catching it here turns a bad file into a recoverable
/// `Err` the caller can skip-and-log, and lets table extraction fall back to an
/// empty result while preserving the page text.
///
/// The closure is wrapped in [`std::panic::AssertUnwindSafe`]: the caught panic
/// originates in a self-contained `pdf_oxide` sort over a local `Vec`, so the
/// borrowed `PdfDocument` is not left in an observably-inconsistent state.
pub(crate) fn guard_oxide_panic<T, E>(
    op: impl FnOnce() -> std::result::Result<T, E>,
    on_panic: impl FnOnce(String) -> E,
) -> std::result::Result<T, E> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(op)) {
        Ok(result) => result,
        Err(payload) => Err(on_panic(panic_message(payload.as_ref()))),
    }
}

/// Extract a human-readable message from a caught panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// Wraps a [`pdf_oxide::PdfDocument`] with convenient constructors that map
/// pdf_oxide errors into [`XbergError::Parsing`].
pub(crate) struct OxideDocument {
    pub doc: pdf_oxide::PdfDocument,
}

impl OxideDocument {
    /// Open a PDF from in-memory bytes with no password. Test-only convenience;
    /// production opens through `open_bytes_with_passwords` so configured
    /// passwords are always consulted.
    #[cfg(test)]
    pub(crate) fn open_bytes(bytes: &[u8]) -> Result<Self> {
        Self::open_bytes_with_passwords(bytes, &[])
    }

    /// Open a PDF from in-memory bytes, trying each candidate password for
    /// encrypted documents.
    ///
    /// pdf_oxide auto-authenticates with the empty password (the common default
    /// for lightly-encrypted PDFs). When that fails and the document is
    /// user-password protected, extraction previously returned an empty success
    /// because `PdfConfig.passwords` was never consulted (xberg-io/xberg#1223).
    /// Each candidate is tried in order; the first that authenticates wins. If
    /// the document is encrypted and none work, an error is returned rather than
    /// silently yielding empty text.
    pub(crate) fn open_bytes_with_passwords(bytes: &[u8], passwords: &[String]) -> Result<Self> {
        let doc = pdf_oxide::PdfDocument::from_bytes(bytes.to_vec()).map_err(|e| XbergError::Parsing {
            message: format!("pdf_oxide: failed to load bytes: {e}"),
            source: None,
        })?;

        let opened = doc.authenticate(b"").unwrap_or(false);
        if !opened {
            let mut authenticated = false;
            for password in passwords {
                if doc.authenticate(password.as_bytes()).unwrap_or(false) {
                    authenticated = true;
                    break;
                }
            }
            if !authenticated {
                return Err(XbergError::Parsing {
                    message: if passwords.is_empty() {
                        "PDF is encrypted and requires a password; set pdf_options.passwords".to_string()
                    } else {
                        "PDF is encrypted and none of the supplied passwords authenticated".to_string()
                    },
                    source: None,
                });
            }
        }

        Ok(Self { doc })
    }
}
