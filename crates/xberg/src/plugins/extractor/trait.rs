//! Document extractor plugin trait.
//!
//! This module defines the trait for implementing custom document extractors.

use crate::Result;
use crate::core::config::{ExtractInput, ExtractInputKind, ExtractionConfig};
use crate::plugins::Plugin;
use crate::types::ExtractedDocument;
use crate::types::internal::InternalDocument;
use async_trait::async_trait;
use std::path::Path;

#[cfg(not(feature = "tokio-runtime"))]
use crate::XbergError;

/// Trait for document extractor plugins.
///
/// Implement this trait to add support for new document formats or override
/// built-in extraction behavior. Foreign-language bindings expose the
/// [`DocumentExtractor::extract`] method, which accepts [`ExtractInput`] and
/// returns an [`ExtractedDocument`].
///
/// # Priority System
///
/// When multiple extractors support the same MIME type, the registry selects
/// the extractor with the highest priority value. Use this to:
/// - Override built-in extractors (priority > 50)
/// - Provide fallback extractors (priority < 50)
/// - Implement specialized extractors for specific use cases
///
/// Default priority is 50.
///
/// # Thread Safety
///
/// Extractors must be thread-safe (`Send + Sync`) to support concurrent extraction.
///
/// # Example
///
/// ```rust
/// use xberg::plugins::{Plugin, DocumentExtractor};
/// use xberg::{ExtractInput, ExtractionConfig, ExtractedDocument, Result};
/// use async_trait::async_trait;
///
/// struct CustomTextExtractor;
///
/// impl Plugin for CustomTextExtractor {
///     fn name(&self) -> &str { "custom-text" }
///     fn version(&self) -> String { "1.0.0".to_string() }
///     fn initialize(&self) -> Result<()> { Ok(()) }
///     fn shutdown(&self) -> Result<()> { Ok(()) }
/// }
///
/// #[async_trait]
/// impl DocumentExtractor for CustomTextExtractor {
///     async fn extract(&self, input: ExtractInput, _config: &ExtractionConfig)
///         -> Result<ExtractedDocument> {
///         let bytes = input.bytes.unwrap_or_default();
///         // `ExtractedDocument` has private internal fields, so a struct literal with
///         // `..Default::default()` does not compile outside the crate. Build a default
///         // and assign the public fields instead.
///         let mut document = ExtractedDocument::default();
///         document.content = String::from_utf8_lossy(&bytes).to_string();
///         document.mime_type = "text/plain".into();
///         Ok(document)
///     }
///
///     fn supported_mime_types(&self) -> &[&str] {
///         &["text/plain"]
///     }
/// }
/// ```
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait DocumentExtractor: Plugin {
    /// Binding-safe extraction entry point for foreign-language plugin bridges.
    ///
    /// Accepts the same unified input shape as the public extraction API and
    /// returns one extracted document result.
    async fn extract(&self, input: ExtractInput, config: &ExtractionConfig) -> Result<ExtractedDocument>;

    /// Get the list of MIME types supported by this extractor.
    ///
    /// Can include exact MIME types and prefix patterns:
    /// - Exact: `"application/pdf"`, `"text/plain"`
    /// - Prefix: `"image/*"` (matches any image type)
    ///
    /// # Returns
    ///
    /// A slice of MIME type strings.
    fn supported_mime_types(&self) -> &[&str];

    /// Get the priority of this extractor.
    ///
    /// Higher priority extractors are preferred when multiple extractors
    /// support the same MIME type.
    ///
    /// # Priority Guidelines
    ///
    /// - **0-25**: Fallback/low-quality extractors
    /// - **26-49**: Alternative extractors
    /// - **50**: Default priority (built-in extractors)
    /// - **51-75**: Premium/enhanced extractors
    /// - **76-100**: Specialized/high-priority extractors
    ///
    /// # Returns
    ///
    /// Priority value (default: 50)
    fn priority(&self) -> i32 {
        50
    }

    /// Optional: Check if this extractor can handle a specific file.
    ///
    /// Allows for more sophisticated detection beyond MIME types.
    /// Defaults to `true` (rely on MIME type matching).
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the file to check
    /// * `mime_type` - Detected MIME type
    ///
    /// # Returns
    ///
    /// `true` if the extractor can handle this file, `false` otherwise.
    fn can_handle(&self, _path: &Path, _mime_type: &str) -> bool {
        true
    }
}

/// Low-level extraction capability used by native Rust extractors.
///
/// Native extractors implement this to produce the pipeline's
/// [`InternalDocument`] representation; the blanket impl below derives the
/// public [`DocumentExtractor`] surface from it. Exposed for white-box tests
/// and advanced consumers that need the pre-derivation document.
///
/// Skipped by the binding surface: it returns the internal-only
/// [`InternalDocument`], which is not part of the public binding API.
#[cfg_attr(alef, alef(skip))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait InternalDocumentExtractor: Plugin {
    /// Extract an in-memory payload into the pipeline representation.
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument>;

    /// Extract a path into the pipeline representation.
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        #[cfg(feature = "tokio-runtime")]
        {
            use crate::core::io;
            let bytes = io::open_file_bytes(path)?;
            self.extract_content(&bytes, mime_type, config).await
        }
        #[cfg(not(feature = "tokio-runtime"))]
        {
            let _ = (path, mime_type, config);
            Err(XbergError::Other(
                "Path extraction requires the tokio-runtime feature".to_string(),
            ))
        }
    }

    /// Get the list of MIME types supported by this extractor.
    fn supported_mime_types(&self) -> &[&str];

    /// Get the priority of this extractor.
    fn priority(&self) -> i32 {
        50
    }

    /// Check if this extractor can handle a specific path and MIME type.
    fn can_handle(&self, _path: &Path, _mime_type: &str) -> bool {
        true
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl<T> DocumentExtractor for T
where
    T: InternalDocumentExtractor + ?Sized,
{
    async fn extract(&self, input: ExtractInput, config: &ExtractionConfig) -> Result<ExtractedDocument> {
        let doc = match input.kind {
            ExtractInputKind::Bytes => {
                let bytes = input.bytes.ok_or_else(|| {
                    crate::XbergError::validation(
                        "document extractor input kind 'bytes' requires the 'bytes' field".to_string(),
                    )
                })?;
                let mime_type = input.mime_type.as_deref().unwrap_or("application/octet-stream");
                InternalDocumentExtractor::extract_content(self, &bytes, mime_type, config).await?
            }
            ExtractInputKind::Uri => {
                let uri = input.uri.ok_or_else(|| {
                    crate::XbergError::validation(
                        "document extractor input kind 'uri' requires the 'uri' field".to_string(),
                    )
                })?;
                let mime_type = input.mime_type.as_deref().unwrap_or("application/octet-stream");
                InternalDocumentExtractor::extract_path(self, Path::new(&uri), mime_type, config).await?
            }
        };

        Ok(crate::extraction::derive::derive_extraction_result(
            doc,
            config.include_document_structure,
            config.output_format.clone(),
        ))
    }

    fn supported_mime_types(&self) -> &[&str] {
        InternalDocumentExtractor::supported_mime_types(self)
    }

    fn priority(&self) -> i32 {
        InternalDocumentExtractor::priority(self)
    }

    fn can_handle(&self, path: &Path, mime_type: &str) -> bool {
        InternalDocumentExtractor::can_handle(self, path, mime_type)
    }
}
