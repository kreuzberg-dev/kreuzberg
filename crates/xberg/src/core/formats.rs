//! Format field validation and metadata.
//!
//! This module provides a compile-time registry of known format fields used across
//! document extractors. It serves as the single source of truth for format validation
//! across all language bindings (Rust, Python, TypeScript, Ruby, Java, Go).
//!
//! # Known Format Fields
//!
//! The registry contains 55 standardized format fields organized by category:
//! - **Document Properties**: title, author, keywords, creator, producer, etc.
//! - **Dates**: creation_date, modification_date
//! - **Pagination**: page_count
//! - **Email Metadata**: from_email, from_name, to_emails, cc_emails, bcc_emails, message_id
//! - **Attachments**: attachments
//! - **Descriptions**: description, summary
//! - **Typography**: fonts
//! - **Archive/Compression**: format, file_count, file_list, total_size, compressed_size
//! - **Images**: width, height
//! - **Content Metrics**: element_count, unique_elements, line_count, word_count, character_count
//! - **HTML Structure**: headers, links, code_blocks
//! - **Meta Tags**: canonical, base_href, og_*, twitter_*, link_*
//! - **OCR**: psm, output_format
//! - **Tables**: table_count, table_rows, table_cols
//!
//! # Example
//!
//! ```ignore
//! use xberg::core::formats::{KNOWN_FORMATS, is_valid_format_field};
//!
//! assert!(is_valid_format_field("title"));
//! assert!(!is_valid_format_field("invalid_field"));
//! assert_eq!(KNOWN_FORMATS.len(), 55);
//! ```

#[cfg(test)]
use ahash::AHashSet;
#[cfg(test)]
use std::sync::LazyLock;

/// All known format field names across all extractors.
///
/// This is a compile-time constant array of standardized field names used by document
/// extractors. Each binding (Python, TypeScript, Ruby, Java, Go) should reference this
/// as the single source of truth for format field validation.
///
/// Format fields are organized by document type:
/// - PDF/Office: title, author, creation_date, page_count, etc.
/// - Email: from_email, to_emails, cc_emails, bcc_emails, etc.
/// - Web: og_title, twitter_card, canonical, headers, links, etc.
/// - Images: width, height, format
/// - Archives: file_count, file_list, total_size, etc.
#[cfg(test)]
pub(crate) const KNOWN_FORMATS: &[&str] = &[
    "title",
    "author",
    "keywords",
    "creator",
    "producer",
    "creation_date",
    "modification_date",
    "page_count",
    "from_email",
    "from_name",
    "to_emails",
    "cc_emails",
    "bcc_emails",
    "message_id",
    "attachments",
    "description",
    "summary",
    "fonts",
    "format",
    "file_count",
    "file_list",
    "total_size",
    "compressed_size",
    "width",
    "height",
    "element_count",
    "unique_elements",
    "line_count",
    "word_count",
    "character_count",
    "headers",
    "links",
    "code_blocks",
    "canonical",
    "base_href",
    "og_title",
    "og_description",
    "og_image",
    "og_url",
    "og_type",
    "og_site_name",
    "twitter_card",
    "twitter_title",
    "twitter_description",
    "twitter_image",
    "twitter_site",
    "twitter_creator",
    "link_author",
    "link_license",
    "link_alternate",
    "psm",
    "output_format",
    "table_count",
    "table_rows",
    "table_cols",
];

/// Cached format field set for fast O(1) lookups.
///
/// Uses AHashSet for its excellent cache locality and performance characteristics
/// with string keys. Built lazily on first use with minimal overhead.
#[cfg(test)]
static FORMAT_FIELD_SET: LazyLock<AHashSet<&'static str>> = LazyLock::new(|| KNOWN_FORMATS.iter().copied().collect());

/// Validates whether a field name is in the known formats registry.
///
/// This uses a pre-built hash set for O(1) lookups instead of linear search,
/// providing significant performance improvements for repeated validations.
///
/// # Arguments
///
/// * `field` - The field name to validate
///
/// # Returns
///
/// `true` if the field is in KNOWN_FORMATS, `false` otherwise.
///
/// # Example
///
/// Not run as a doctest: this helper is `#[cfg(test)] pub(crate)`, so it does not
/// exist in any build a downstream crate can link against.
///
/// ```ignore
/// use xberg::core::formats::is_valid_format_field;
///
/// assert!(is_valid_format_field("title"));
/// assert!(is_valid_format_field("creation_date"));
/// assert!(!is_valid_format_field("invalid_field"));
/// ```
#[cfg(test)]
#[inline]
pub(crate) fn is_valid_format_field(field: &str) -> bool {
    FORMAT_FIELD_SET.contains(field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_known_formats_count() {
        assert_eq!(KNOWN_FORMATS.len(), 55, "Expected 55 known format fields");
    }

    #[test]
    fn test_known_formats_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for field in KNOWN_FORMATS {
            assert!(seen.insert(field), "Duplicate format field found: {}", field);
        }
    }

    #[test]
    fn test_is_valid_format_field_true_cases() {
        assert!(is_valid_format_field("title"));
        assert!(is_valid_format_field("author"));
        assert!(is_valid_format_field("creation_date"));
        assert!(is_valid_format_field("page_count"));
        assert!(is_valid_format_field("from_email"));
        assert!(is_valid_format_field("og_title"));
        assert!(is_valid_format_field("twitter_card"));
    }

    #[test]
    fn test_is_valid_format_field_false_cases() {
        assert!(!is_valid_format_field("invalid_field"));
        assert!(!is_valid_format_field("unknown_metadata"));
        assert!(!is_valid_format_field(""));
        assert!(!is_valid_format_field("TITLE"));
        assert!(!is_valid_format_field("title "));
    }

    #[test]
    fn test_all_document_property_fields() {
        let doc_fields = ["title", "author", "keywords", "creator", "producer"];
        for field in &doc_fields {
            assert!(is_valid_format_field(field), "Missing field: {}", field);
        }
    }

    #[test]
    fn test_all_email_fields() {
        let email_fields = [
            "from_email",
            "from_name",
            "to_emails",
            "cc_emails",
            "bcc_emails",
            "message_id",
            "attachments",
        ];
        for field in &email_fields {
            assert!(is_valid_format_field(field), "Missing email field: {}", field);
        }
    }

    #[test]
    fn test_all_web_meta_fields() {
        let web_fields = [
            "og_title",
            "og_description",
            "og_image",
            "og_url",
            "og_type",
            "og_site_name",
            "twitter_card",
            "twitter_title",
            "twitter_description",
            "twitter_image",
            "twitter_site",
            "twitter_creator",
            "canonical",
            "base_href",
        ];
        for field in &web_fields {
            assert!(is_valid_format_field(field), "Missing web field: {}", field);
        }
    }

    #[test]
    fn test_all_table_fields() {
        let table_fields = ["table_count", "table_rows", "table_cols"];
        for field in &table_fields {
            assert!(is_valid_format_field(field), "Missing table field: {}", field);
        }
    }
}
