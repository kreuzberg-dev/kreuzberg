//! Shared helpers for surfacing degradation to callers without flooding
//! `processing_warnings`.
//!
//! External dependencies (OCR engines, layout/model runtimes, archive readers)
//! commonly emit the *same* warning for every page or every item. Collecting
//! them verbatim buries the signal under duplicates. These helpers collapse
//! identical `(source, message)` warnings to a single entry so a caller sees
//! one line per distinct problem, not one per page — generalizing the per-page
//! dedup introduced for the paddle-ocr language warning (#1346).
//!
//! # Convention: when an extractor must emit a `ProcessingWarning` (#171)
//!
//! An extractor that returns `Ok` has told the caller "this is the document".
//! Whenever that is only *partly* true, the gap must be named in
//! `InternalDocument::processing_warnings` — the caller has no other way to
//! distinguish a fully-extracted document from a half-extracted one.
//!
//! Emit a warning at every point where the extractor:
//!
//! 1. **stops early** — a parse loop that exits on a malformed event
//!    (`Err(_) => break`) abandons every byte after that point. Use
//!    [`push_truncated_parse_warning`].
//! 2. **skips an item** — an archive member, attachment, sheet, slide or page
//!    that could not be read is dropped from the output. Name the items with
//!    [`format_entry_list`] and push via [`push_warning`]; an unnamed count is
//!    not actionable (#115).
//! 3. **degrades** — a fallback path produced lower-fidelity output than the
//!    primary path (OCR instead of native text, plain text instead of tables).
//!
//! Do **not** emit a warning for a deliberate, documented filter (e.g. content
//! excluded by config) unless the user could plausibly believe that content was
//! extracted.
//!
//! Rules for the warning itself:
//!
//! - `source` is a stable, lowercase, extractor-scoped identifier — `"xml"`,
//!   `"fictionbook"`, `"archive"`. It is a free-form string on the wire, so new
//!   values carry no binding cost, but existing values are effectively public:
//!   do not rename them.
//! - `message` states what was lost, not merely that something went wrong, and
//!   names the offending item where one exists.
//! - Always push through [`push_warning`] / [`push_warning_deduped`] so a
//!   per-item failure repeated across a large document collapses to one entry.
//! - Never add a field to `ProcessingWarning` — the struct is binding content.
//!   Everything belongs in `message`.

// Every consumer of these helpers is behind an extractor feature flag
// (`xml`, `archives`, `office`, `pdf`, ...), so a build that enables none of
// them leaves parts of this module unused. ~keep
#![allow(dead_code)]

use crate::types::ProcessingWarning;
use std::borrow::Cow;

/// Maximum number of item names embedded in a single warning message before the
/// list is elided; keeps a warning readable for an archive with thousands of
/// unreadable members while still naming the first offenders.
const MAX_NAMED_ENTRIES: usize = 10;

/// Build a `ProcessingWarning` attributed to `source`.
pub(crate) fn warning(source: &'static str, message: impl Into<String>) -> ProcessingWarning {
    ProcessingWarning {
        source: Cow::Borrowed(source),
        message: Cow::Owned(message.into()),
    }
}

/// Record `message` against `source`, collapsing an exact repeat of the same
/// `(source, message)` pair.
pub(crate) fn push_warning(accumulated: &mut Vec<ProcessingWarning>, source: &'static str, message: impl Into<String>) {
    push_warning_deduped(accumulated, warning(source, message));
}

/// Record that a parse loop abandoned the rest of its input.
///
/// `stage` names the thing being parsed in the extractor's own vocabulary
/// ("document body", "metadata", "embedded images"); `cause` is the underlying
/// parser error.
pub(crate) fn push_truncated_parse_warning(
    accumulated: &mut Vec<ProcessingWarning>,
    source: &'static str,
    stage: &str,
    cause: &dyn std::fmt::Display,
) {
    let message = format!(
        "Parsing of {stage} stopped early at a malformed XML event; \
         the remaining content was not extracted (cause: {cause})"
    );
    push_warning(accumulated, source, message);
}

/// Record that the input ran out while elements were still open.
///
/// A markup parser that is told not to check end names cannot fail on a
/// truncated document — it simply reaches EOF with a non-empty element stack and
/// returns whatever it managed to read. That is the silent truncation callers
/// cannot otherwise detect (#134). `unclosed` is outermost-first.
pub(crate) fn push_unclosed_elements_warning(
    accumulated: &mut Vec<ProcessingWarning>,
    source: &'static str,
    unclosed: &[String],
) {
    if unclosed.is_empty() {
        return;
    }
    let message = format!(
        "Input ended with {} unclosed element{} ({}); the document is truncated \
         and trailing content was not extracted",
        unclosed.len(),
        if unclosed.len() == 1 { "" } else { "s" },
        format_entry_list(unclosed)
    );
    push_warning(accumulated, source, message);
}

/// Record that the input could not be decoded as UTF-8 and was read lossily.
///
/// `String::from_utf8_lossy` — and the `utf8_validation` fallbacks built on it —
/// substitute U+FFFD for every byte they cannot interpret, then extraction
/// continues and returns `Ok`. Without this warning a mojibake'd document is
/// indistinguishable from a clean one, which is exactly the gap #171 is about.
///
/// `subject` names the input in the extractor's own vocabulary ("HTML source",
/// "RTF source"). Emit this only when the decode actually fell back — a document
/// that is valid UTF-8 must not produce it.
pub(crate) fn push_lossy_decode_warning(accumulated: &mut Vec<ProcessingWarning>, source: &'static str, subject: &str) {
    let message = format!(
        "The {subject} is not valid UTF-8; the undecodable bytes were replaced with the Unicode \
         replacement character, so those characters are missing from the extracted text. \
         Re-encode the input as UTF-8 to recover them"
    );
    push_warning(accumulated, source, message);
}

/// Render item names for embedding in a warning message: comma-separated, and
/// elided after [`MAX_NAMED_ENTRIES`] with a count of the remainder.
pub(crate) fn format_entry_list(names: &[String]) -> String {
    if names.len() <= MAX_NAMED_ENTRIES {
        return names.join(", ");
    }
    format!(
        "{}, and {} more",
        names[..MAX_NAMED_ENTRIES].join(", "),
        names.len() - MAX_NAMED_ENTRIES
    )
}

/// Push `warning` into `accumulated` unless an identical `(source, message)`
/// entry is already present. Order of first occurrence is preserved.
pub(crate) fn push_warning_deduped(accumulated: &mut Vec<ProcessingWarning>, warning: ProcessingWarning) {
    if !accumulated
        .iter()
        .any(|existing| existing.source == warning.source && existing.message == warning.message)
    {
        accumulated.push(warning);
    }
}

/// Append `new` warnings into `accumulated`, skipping any whose
/// `(source, message)` pair already appears.
///
/// A backend that warns about its configuration (e.g. paddle-ocr's unsupported
/// language warning, or a per-page model degradation) emits the same warning on
/// every page; one copy per document is enough.
#[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
pub(crate) fn dedup_extend_warnings(accumulated: &mut Vec<ProcessingWarning>, new: Vec<ProcessingWarning>) {
    for warning in new {
        push_warning_deduped(accumulated, warning);
    }
}

#[cfg(test)]
mod convention_tests {
    use super::*;

    #[test]
    fn should_collapse_repeated_push_warning_for_same_source_and_message() {
        let mut accumulated = Vec::new();
        push_warning(&mut accumulated, "xml", "lost the tail");
        push_warning(&mut accumulated, "xml", "lost the tail");

        assert_eq!(accumulated.len(), 1);
        assert_eq!(accumulated[0].source, "xml");
        assert_eq!(accumulated[0].message, "lost the tail");
    }

    #[test]
    fn should_name_the_cause_and_stage_in_truncation_warning() {
        let mut accumulated = Vec::new();
        push_truncated_parse_warning(&mut accumulated, "fictionbook", "the document body", &"boom");

        assert_eq!(accumulated[0].source, "fictionbook");
        assert_eq!(
            accumulated[0].message,
            "Parsing of the document body stopped early at a malformed XML event; \
             the remaining content was not extracted (cause: boom)"
        );
    }

    #[test]
    fn should_elide_entry_list_past_the_named_maximum() {
        let short: Vec<String> = vec!["a.txt".into(), "b.txt".into()];
        assert_eq!(format_entry_list(&short), "a.txt, b.txt");

        let long: Vec<String> = (0..13).map(|i| format!("f{i}")).collect();
        assert_eq!(
            format_entry_list(&long),
            "f0, f1, f2, f3, f4, f5, f6, f7, f8, f9, and 3 more"
        );
    }
}

#[cfg(all(test, feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
mod tests {
    use super::*;
    use std::borrow::Cow;

    fn warning(source: &'static str, message: &str) -> ProcessingWarning {
        ProcessingWarning {
            source: Cow::Borrowed(source),
            message: Cow::Owned(message.to_string()),
        }
    }

    #[test]
    fn dedup_extend_drops_identical_keeps_distinct() {
        let mut accumulated = vec![warning("paddle-ocr", "a")];
        dedup_extend_warnings(
            &mut accumulated,
            vec![warning("paddle-ocr", "a"), warning("paddle-ocr", "b")],
        );
        dedup_extend_warnings(
            &mut accumulated,
            vec![warning("paddle-ocr", "a"), warning("paddle-ocr", "b")],
        );

        let messages: Vec<&str> = accumulated.iter().map(|w| w.message.as_ref()).collect();
        assert_eq!(messages, vec!["a", "b"]);
    }

    #[test]
    fn same_message_different_source_is_kept() {
        let mut accumulated = vec![warning("layout", "failed")];
        push_warning_deduped(&mut accumulated, warning("ocr", "failed"));
        push_warning_deduped(&mut accumulated, warning("layout", "failed"));

        assert_eq!(accumulated.len(), 2, "distinct sources must not collapse");
    }
}
