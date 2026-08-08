//! Validation for caller-supplied cache namespaces.
//!
//! A namespace is user-controlled input (it reaches the library from
//! `ExtractionConfig::cache_namespace`, which is settable over the REST API, the
//! CLI, and every language binding) and it is used verbatim as a directory
//! component beneath the cache root. Without validation a namespace of `..`,
//! `../../etc`, or `/etc` escapes the cache root and makes the library create —
//! and later delete — directories anywhere the process can write.
//!
//! Validation therefore uses an **allowlist** of characters that are safe as a
//! single path component on every supported platform, not a denylist of known
//! traversal spellings. A denylist cannot cover `..`, `.%2e`, backslashes on
//! Windows, NUL truncation, or Unicode look-alikes; the allowlist rejects all of
//! them by construction because none of those bytes are on it.

use crate::error::{Result, XbergError};

/// Maximum length of a cache namespace, in bytes.
///
/// Comfortably fits a UUID, a tenant slug, or a project identifier while staying
/// well inside the 255-byte component limit of every common filesystem.
const MAX_NAMESPACE_LEN: usize = 128;

/// Returns `true` if `byte` is allowed inside a cache namespace.
///
/// The allowlist is ASCII letters, ASCII digits, `-`, `_` and `.`. Every path
/// separator (`/`, `\`), drive-letter colon, NUL and whitespace byte is absent,
/// so a validated namespace is always exactly one path component.
const fn is_allowed_namespace_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
}

/// Validate a caller-supplied cache namespace.
///
/// Returns `Ok(())` only for a non-empty, at-most-[`MAX_NAMESPACE_LEN`]-byte
/// string built entirely from [`is_allowed_namespace_byte`] characters that does
/// not begin with `.`.
///
/// Rejecting a leading `.` removes the `.` and `..` relative components (and
/// hidden directories) while still allowing dots inside a name such as
/// `acme.corp`.
pub(crate) fn validate_namespace(namespace: &str) -> Result<()> {
    if namespace.is_empty() {
        return Err(XbergError::validation("Cache namespace must not be empty".to_string()));
    }

    if namespace.len() > MAX_NAMESPACE_LEN {
        return Err(XbergError::validation(format!(
            "Cache namespace must be at most {} bytes, got {}",
            MAX_NAMESPACE_LEN,
            namespace.len()
        )));
    }

    if let Some(byte) = namespace.bytes().find(|byte| !is_allowed_namespace_byte(*byte)) {
        return Err(XbergError::validation(format!(
            "Cache namespace contains the disallowed byte {:?}; allowed characters are \
             ASCII letters, ASCII digits, '-', '_' and '.'",
            byte as char
        )));
    }

    if namespace.starts_with('.') {
        return Err(XbergError::validation(
            "Cache namespace must not start with '.' (reserved for relative path components)".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_accept_plain_tenant_namespace() {
        assert!(validate_namespace("invoices").is_ok());
        assert!(validate_namespace("tenant-123").is_ok());
        assert!(validate_namespace("acme_corp").is_ok());
        assert!(validate_namespace("acme.corp").is_ok());
        assert!(validate_namespace("6ab3ccc3-ca20-46d4-9592-f18f80497290").is_ok());
    }

    #[test]
    fn should_reject_parent_directory_component() {
        let error = validate_namespace("..").expect_err("'..' must be rejected");
        assert!(
            error.to_string().contains("must not start with '.'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn should_reject_current_directory_component() {
        assert!(validate_namespace(".").is_err(), "'.' must be rejected");
    }

    #[test]
    fn should_reject_relative_traversal_path() {
        assert!(validate_namespace("../../etc").is_err());
        assert!(validate_namespace("a/../../b").is_err());
        assert!(validate_namespace("nested/child").is_err());
    }

    #[test]
    fn should_reject_absolute_paths() {
        assert!(validate_namespace("/etc/passwd").is_err());
        assert!(validate_namespace("/").is_err());
    }

    #[test]
    fn should_reject_windows_separators_and_drive_letters() {
        assert!(validate_namespace("..\\..\\windows").is_err());
        assert!(validate_namespace("C:\\temp").is_err());
    }

    #[test]
    fn should_reject_empty_namespace() {
        let error = validate_namespace("").expect_err("empty namespace must be rejected");
        assert!(error.to_string().contains("must not be empty"), "unexpected: {error}");
    }

    #[test]
    fn should_reject_namespace_longer_than_the_limit() {
        let at_limit = "a".repeat(MAX_NAMESPACE_LEN);
        assert!(validate_namespace(&at_limit).is_ok(), "exactly at the limit is allowed");

        let over_limit = "a".repeat(MAX_NAMESPACE_LEN + 1);
        let error = validate_namespace(&over_limit).expect_err("over the limit must be rejected");
        assert!(error.to_string().contains("at most 128 bytes"), "unexpected: {error}");
    }

    #[test]
    fn should_reject_nul_and_whitespace() {
        assert!(validate_namespace("evil\0name").is_err());
        assert!(validate_namespace("two words").is_err());
        assert!(validate_namespace("line\nbreak").is_err());
    }

    #[test]
    fn should_reject_non_ascii() {
        assert!(validate_namespace("café").is_err());
        assert!(validate_namespace("\u{202e}gpj.exe").is_err());
    }

    #[test]
    fn should_allow_dots_that_are_not_leading() {
        assert!(
            validate_namespace("a..b").is_ok(),
            "'..' inside a name is one component"
        );
        assert!(validate_namespace("v1.0.14").is_ok());
    }
}
