//! Built-in document extractors.
//!
//! This module contains the default extractors that ship with Xberg.
//! All extractors implement the `DocumentExtractor` plugin trait.

use crate::Result;
#[cfg(any(
    feature = "archives",
    feature = "email",
    feature = "excel",
    feature = "excel-wasm",
    feature = "html",
    feature = "tree-sitter",
    feature = "xml",
))]
use crate::core::config::ExtractionConfig;
use crate::plugins::registry::get_document_extractor_registry;

#[cfg(any(
    feature = "archives",
    feature = "email",
    feature = "excel",
    feature = "excel-wasm",
    feature = "html",
    feature = "tree-sitter",
    feature = "xml",
))]
use crate::types::internal::InternalDocument;
use once_cell::sync::OnceCell;
use std::sync::Arc;

/// Trait for extractors that share a blocking parser implementation.
///
/// This trait defines a synchronous extraction interface for formats whose async
/// extractor can delegate to a local blocking parser.
///
/// # Implementation
///
/// Extractors can implement this trait in addition to
/// `InternalDocumentExtractor` when their parser does not need an async runtime.
///
/// # MIME Type Validation
///
/// The `mime_type` parameter is guaranteed to be already validated.
///
/// # Example
///
/// ```rust,ignore
/// impl SyncExtractor for PlainTextExtractor {
///     fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
///         let text = String::from_utf8_lossy(content).to_string();
///         Ok(InternalDocument::from_text(text, mime_type))
///     }
/// }
/// ```
#[cfg(any(
    feature = "archives",
    feature = "email",
    feature = "excel",
    feature = "excel-wasm",
    feature = "html",
    feature = "tree-sitter",
    feature = "xml",
))]
pub(crate) trait SyncExtractor {
    /// Extract content from a byte array synchronously.
    ///
    /// This method performs extraction without requiring an async runtime.
    ///
    /// # Arguments
    ///
    /// * `content` - Raw document bytes
    /// * `mime_type` - MIME type of the document (already validated)
    /// * `config` - Extraction configuration
    ///
    /// # Returns
    ///
    /// An `InternalDocument` containing the extracted elements, metadata, and tables.
    fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument>;
}

#[cfg(feature = "tree-sitter")]
pub mod code;

pub mod csv;
pub mod structured;
pub mod text;

pub mod djot_format;
pub mod frontmatter_utils;

pub(crate) mod annotation_utils;
pub(crate) mod markdown_utils;

pub mod security;
#[cfg(test)]
mod security_tests;

#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
pub mod image;

#[cfg(feature = "qr-codes")]
pub mod qr;

#[cfg(feature = "archives")]
pub mod archive;

#[cfg(feature = "transcription")]
pub mod transcription;

#[cfg(feature = "email")]
pub mod email;

#[cfg(feature = "email")]
pub mod pst;

#[cfg(any(feature = "excel", feature = "excel-wasm"))]
pub mod excel;

#[cfg(feature = "hwp")]
pub mod hwp;

#[cfg(feature = "hwpx")]
pub mod hwpx;

#[cfg(feature = "wordperfect")]
pub mod wordperfect;

#[cfg(feature = "iwork")]
pub mod iwork;

#[cfg(feature = "html")]
pub mod html;

#[cfg(feature = "office")]
pub mod bibtex;

#[cfg(feature = "office")]
pub mod citation;

#[cfg(feature = "office")]
pub mod doc;

#[cfg(feature = "office")]
pub mod dbf;

#[cfg(feature = "office")]
pub mod docx;

#[cfg(feature = "office")]
pub mod epub;

#[cfg(feature = "office")]
pub mod fictionbook;

pub mod doctags;

pub mod markdown;

#[cfg(feature = "mdx")]
pub mod mdx;

#[cfg(feature = "office")]
pub mod rst;

#[cfg(feature = "office")]
pub mod latex;

#[cfg(feature = "notebook")]
pub mod jupyter;

#[cfg(feature = "office")]
pub mod orgmode;

#[cfg(feature = "office")]
pub mod odp;

#[cfg(feature = "office")]
pub mod odt;

#[cfg(feature = "office")]
pub mod opml;

#[cfg(feature = "office")]
pub mod typst;

#[cfg(feature = "xml")]
pub mod jats;

#[cfg(feature = "pdf")]
pub mod pdf;

#[cfg(feature = "office")]
pub mod ppt;

#[cfg(feature = "office")]
pub mod pptx;

#[cfg(feature = "office")]
pub mod rtf;

#[cfg(feature = "xml")]
pub mod xml;

#[cfg(feature = "xml")]
pub mod docbook;

#[cfg(feature = "tree-sitter")]
pub use code::CodeExtractor;

pub use csv::CsvExtractor;
pub use doctags::DocTagsExtractor;
pub use markdown::MarkdownExtractor;
pub use structured::StructuredExtractor;
pub use text::PlainTextExtractor;

#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
pub use image::ImageExtractor;

#[cfg(feature = "archives")]
pub use archive::{GzipExtractor, SevenZExtractor, TarExtractor, ZipExtractor};

#[cfg(feature = "email")]
pub use email::EmailExtractor;

#[cfg(feature = "email")]
pub use pst::PstExtractor;

#[cfg(any(feature = "excel", feature = "excel-wasm"))]
pub use excel::ExcelExtractor;

#[cfg(feature = "hwp")]
pub use hwp::HwpExtractor;

#[cfg(feature = "hwpx")]
pub use hwpx::HwpxExtractor;

#[cfg(feature = "wordperfect")]
pub use wordperfect::WordPerfectExtractor;

#[cfg(feature = "iwork")]
pub use iwork::{keynote::KeynoteExtractor, numbers::NumbersExtractor, pages::PagesExtractor};

#[cfg(feature = "html")]
pub use html::HtmlExtractor;

#[cfg(feature = "office")]
pub use bibtex::BibtexExtractor;

#[cfg(feature = "office")]
pub use citation::CitationExtractor;

#[cfg(feature = "office")]
pub use dbf::DbfExtractor;

#[cfg(feature = "office")]
pub use doc::DocExtractor;

#[cfg(feature = "office")]
pub use docx::DocxExtractor;

#[cfg(feature = "office")]
pub use epub::EpubExtractor;

#[cfg(feature = "office")]
pub use fictionbook::FictionBookExtractor;

pub use djot_format::DjotExtractor;

#[cfg(feature = "mdx")]
pub use mdx::MdxExtractor;

#[cfg(feature = "office")]
pub use rst::RstExtractor;

#[cfg(feature = "office")]
pub use latex::LatexExtractor;

#[cfg(feature = "notebook")]
pub use jupyter::JupyterExtractor;

#[cfg(feature = "office")]
pub use orgmode::OrgModeExtractor;

#[cfg(feature = "office")]
pub use odp::OdpExtractor;
#[cfg(feature = "office")]
pub use odt::OdtExtractor;

#[cfg(feature = "xml")]
pub use jats::JatsExtractor;

#[cfg(feature = "office")]
pub use opml::OpmlExtractor;

#[cfg(feature = "office")]
pub use typst::TypstExtractor;

#[cfg(feature = "pdf")]
pub use pdf::PdfExtractor;

#[cfg(feature = "office")]
pub use ppt::PptExtractor;

#[cfg(feature = "office")]
pub use pptx::PptxExtractor;

#[cfg(feature = "office")]
pub use rtf::RtfExtractor;

#[cfg(feature = "xml")]
pub use xml::XmlExtractor;

#[cfg(feature = "xml")]
pub use docbook::DocbookExtractor;

#[cfg(feature = "transcription")]
pub use transcription::TranscriptionExtractor;

/// One-time initialization guard for the built-in extractor registry.
///
/// Set to `()` once registration succeeds. If registration fails the cell remains
/// empty, so the next call will retry — unlike `Lazy<Result<()>>` which would
/// permanently cache the error and prevent recovery.
static EXTRACTORS_INITIALIZED: OnceCell<()> = OnceCell::new();

/// Ensure built-in extractors are registered.
///
/// This function is called automatically on first extraction operation.
/// It's safe to call multiple times - registration only happens once,
/// unless the registry was cleared, in which case extractors are re-registered.
pub(crate) fn ensure_initialized() -> Result<()> {
    EXTRACTORS_INITIALIZED.get_or_try_init(register_default_extractors)?;

    let registry = get_document_extractor_registry();
    let registry_guard = registry.read();

    if registry_guard.list().is_empty() {
        drop(registry_guard);
        register_default_extractors()?;
    }

    Ok(())
}

/// Register all built-in extractors with the global registry.
///
/// This function should be called once at application startup to register
/// the default extractors (PlainText, Markdown, XML, etc.).
///
/// **Note:** This is called automatically on first extraction operation.
/// Explicit calling is optional.
///
/// # Example
///
/// ```ignore
/// use xberg::extractors::register_default_extractors;
///
/// # fn main() -> xberg::Result<()> {
/// register_default_extractors()?;
/// # Ok(())
/// # }
/// ```
pub(crate) fn register_default_extractors() -> Result<()> {
    let registry = get_document_extractor_registry();
    let mut registry = registry.write();

    registry.register_internal(Arc::new(PlainTextExtractor::new()))?;
    registry.register_internal(Arc::new(MarkdownExtractor::new()))?;
    registry.register_internal(Arc::new(StructuredExtractor::new()))?;
    registry.register_internal(Arc::new(CsvExtractor::new()))?;
    registry.register_internal(Arc::new(DocTagsExtractor::new()))?;

    #[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
    registry.register_internal(Arc::new(ImageExtractor::new()))?;

    #[cfg(feature = "xml")]
    {
        registry.register_internal(Arc::new(XmlExtractor::new()))?;
        registry.register_internal(Arc::new(JatsExtractor::new()))?;
        registry.register_internal(Arc::new(DocbookExtractor::new()))?;
    }

    #[cfg(feature = "pdf")]
    registry.register_internal(Arc::new(PdfExtractor::new()))?;

    #[cfg(any(feature = "excel", feature = "excel-wasm"))]
    registry.register_internal(Arc::new(ExcelExtractor::new()))?;

    registry.register_internal(Arc::new(DjotExtractor::new()))?;

    #[cfg(feature = "notebook")]
    registry.register_internal(Arc::new(JupyterExtractor::new()))?;

    #[cfg(feature = "office")]
    {
        registry.register_internal(Arc::new(BibtexExtractor::new()))?;
        registry.register_internal(Arc::new(CitationExtractor::new()))?;
        registry.register_internal(Arc::new(EpubExtractor::new()))?;
        registry.register_internal(Arc::new(FictionBookExtractor::new()))?;
        registry.register_internal(Arc::new(RtfExtractor::new()))?;
        registry.register_internal(Arc::new(RstExtractor::new()))?;
        registry.register_internal(Arc::new(LatexExtractor::new()))?;
        registry.register_internal(Arc::new(OrgModeExtractor::new()))?;
        registry.register_internal(Arc::new(OpmlExtractor::new()))?;
        registry.register_internal(Arc::new(TypstExtractor::new()))?;
        registry.register_internal(Arc::new(DocExtractor::new()))?;
        registry.register_internal(Arc::new(DocxExtractor::new()))?;
        registry.register_internal(Arc::new(PptExtractor::new()))?;
        registry.register_internal(Arc::new(PptxExtractor::new()))?;
        registry.register_internal(Arc::new(OdtExtractor::new()))?;
        registry.register_internal(Arc::new(OdpExtractor::new()))?;
        registry.register_internal(Arc::new(DbfExtractor::new()))?;
    }

    #[cfg(feature = "hwp")]
    {
        registry.register_internal(Arc::new(HwpExtractor::new()))?;
    }

    #[cfg(feature = "hwpx")]
    {
        registry.register_internal(Arc::new(HwpxExtractor::new()))?;
    }

    #[cfg(feature = "wordperfect")]
    {
        registry.register_internal(Arc::new(WordPerfectExtractor::new()))?;
    }

    #[cfg(feature = "iwork")]
    {
        registry.register_internal(Arc::new(PagesExtractor::new()))?;
        registry.register_internal(Arc::new(NumbersExtractor::new()))?;
        registry.register_internal(Arc::new(KeynoteExtractor::new()))?;
    }

    #[cfg(feature = "mdx")]
    registry.register_internal(Arc::new(MdxExtractor::new()))?;

    #[cfg(feature = "email")]
    {
        registry.register_internal(Arc::new(EmailExtractor::new()))?;
        registry.register_internal(Arc::new(PstExtractor::new()))?;
    }

    #[cfg(feature = "html")]
    registry.register_internal(Arc::new(HtmlExtractor::new()))?;

    #[cfg(feature = "tree-sitter")]
    registry.register_internal(Arc::new(CodeExtractor::new()))?;

    #[cfg(feature = "archives")]
    {
        registry.register_internal(Arc::new(ZipExtractor::new()))?;
        registry.register_internal(Arc::new(TarExtractor::new()))?;
        registry.register_internal(Arc::new(SevenZExtractor::new()))?;
        registry.register_internal(Arc::new(GzipExtractor::new()))?;
    }

    #[cfg(feature = "transcription")]
    registry.register_internal(Arc::new(TranscriptionExtractor))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_default_extractors() {
        let registry = get_document_extractor_registry();
        {
            let mut reg = registry.write();
            *reg = crate::plugins::registry::DocumentExtractorRegistry::new();
        }

        register_default_extractors().expect("Failed to register extractors");

        let reg = registry.read();
        let extractor_names = reg.list();

        #[allow(unused_mut)]
        let mut expected_count = 6;
        assert!(extractor_names.contains(&"plain-text-extractor".to_string()));
        assert!(extractor_names.contains(&"markdown-extractor".to_string()));
        assert!(extractor_names.contains(&"structured-extractor".to_string()));
        assert!(extractor_names.contains(&"djot-extractor".to_string()));
        assert!(extractor_names.contains(&"csv-extractor".to_string()));
        assert!(extractor_names.contains(&"doctags-extractor".to_string()));

        #[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"image-extractor".to_string()));
        }

        #[cfg(feature = "xml")]
        {
            expected_count += 3;
            assert!(extractor_names.contains(&"xml-extractor".to_string()));
            assert!(extractor_names.contains(&"jats-extractor".to_string()));
            assert!(extractor_names.contains(&"docbook-extractor".to_string()));
        }

        #[cfg(feature = "pdf")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"pdf-extractor".to_string()));
        }

        #[cfg(any(feature = "excel", feature = "excel-wasm"))]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"excel-extractor".to_string()));
        }

        #[cfg(feature = "notebook")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"jupyter-extractor".to_string()));
        }

        #[cfg(feature = "office")]
        {
            expected_count += 17;
            assert!(extractor_names.contains(&"bibtex-extractor".to_string()));
            assert!(extractor_names.contains(&"citation-extractor".to_string()));
            assert!(extractor_names.contains(&"epub-extractor".to_string()));
            assert!(extractor_names.contains(&"fictionbook-extractor".to_string()));
            assert!(extractor_names.contains(&"rtf-extractor".to_string()));
            assert!(extractor_names.contains(&"rst-extractor".to_string()));
            assert!(extractor_names.contains(&"latex-extractor".to_string()));
            assert!(extractor_names.contains(&"orgmode-extractor".to_string()));
            assert!(extractor_names.contains(&"opml-extractor".to_string()));
            assert!(extractor_names.contains(&"typst-extractor".to_string()));
            assert!(extractor_names.contains(&"dbf-extractor".to_string()));
            assert!(extractor_names.contains(&"doc-extractor".to_string()));
            assert!(extractor_names.contains(&"docx-extractor".to_string()));
            assert!(extractor_names.contains(&"ppt-extractor".to_string()));
            assert!(extractor_names.contains(&"pptx-extractor".to_string()));
            assert!(extractor_names.contains(&"odt-extractor".to_string()));
            assert!(extractor_names.contains(&"odp-extractor".to_string()));
        }

        #[cfg(feature = "hwp")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"hwp-extractor".to_string()));
        }

        #[cfg(feature = "hwpx")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"hwpx-extractor".to_string()));
        }

        #[cfg(feature = "wordperfect")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"wordperfect-extractor".to_string()));
        }

        #[cfg(feature = "iwork")]
        {
            expected_count += 3;
            assert!(extractor_names.contains(&"iwork-pages-extractor".to_string()));
            assert!(extractor_names.contains(&"iwork-numbers-extractor".to_string()));
            assert!(extractor_names.contains(&"iwork-keynote-extractor".to_string()));
        }

        #[cfg(feature = "mdx")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"mdx-extractor".to_string()));
        }

        #[cfg(feature = "email")]
        {
            expected_count += 2;
            assert!(extractor_names.contains(&"email-extractor".to_string()));
            assert!(extractor_names.contains(&"pst-extractor".to_string()));
        }

        #[cfg(feature = "html")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"html-extractor".to_string()));
        }

        #[cfg(feature = "tree-sitter")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"code-extractor".to_string()));
        }

        #[cfg(feature = "archives")]
        {
            expected_count += 4;
            assert!(extractor_names.contains(&"zip-extractor".to_string()));
            assert!(extractor_names.contains(&"tar-extractor".to_string()));
            assert!(extractor_names.contains(&"7z-extractor".to_string()));
            assert!(extractor_names.contains(&"gzip-extractor".to_string()));
        }

        #[cfg(feature = "transcription")]
        {
            expected_count += 1;
            assert!(extractor_names.contains(&"transcription".to_string()));
        }

        assert_eq!(
            extractor_names.len(),
            expected_count,
            "Expected {} extractors based on enabled features",
            expected_count
        );
    }

    #[test]
    fn test_ensure_initialized() {
        ensure_initialized().expect("Failed to ensure extractors initialized");
    }
}
