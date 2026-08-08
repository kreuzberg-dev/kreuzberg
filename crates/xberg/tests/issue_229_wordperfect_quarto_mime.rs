//! Regression test for https://github.com/xberg-io/xberg/issues/229
//!
//! `core/mime.rs` advertises `application/wordperfect` as an alias of the
//! WordPerfect canonical MIME type (`application/vnd.wordperfect`), and
//! `application/x-quarto` as an alias of the Quarto canonical MIME type
//! (`text/x-quarto`). `validate_mime_type` accepts alias strings verbatim
//! (it does not normalize them to the canonical form), so any extractor that
//! only registers the canonical MIME leaves the alias string unroutable:
//! `DocumentExtractorRegistry::get_registered` does exact-string lookup and
//! returns `UnsupportedFormat` for a MIME type the library itself claims to
//! support.
//!
//! `WordPerfectExtractor::supported_mime_types()` and
//! `MarkdownExtractor::supported_mime_types()` have both been fixed to also
//! register their declared alias.
//!
//! A survey of the whole static table found five aliases in this state, not
//! two: `application/x-quarto` (text/x-quarto), plus `audio/mp3`
//! (audio/mpeg), `audio/x-m4a` (audio/mp4), `audio/x-wav` (audio/wav) and
//! `video/mpeg` (video/mp4), all of which `TranscriptionExtractor` leaves
//! unclaimed. Those four are tracked separately — see the task for #229.

#[cfg(feature = "wordperfect")]
#[test]
fn wordperfect_alias_mime_resolves_to_wordperfect_extractor() {
    use xberg::extractors::ensure_initialized;
    use xberg::plugins::registry::get_document_extractor_registry;

    ensure_initialized().expect("failed to initialize default extractors");

    let registry = get_document_extractor_registry();
    let registry = registry.read();

    // Canonical MIME type must resolve.
    let canonical = registry
        .get("application/vnd.wordperfect")
        .expect("canonical WordPerfect MIME type must resolve to an extractor, not UnsupportedFormat");
    assert_eq!(canonical.name(), "wordperfect-extractor");

    // The alias declared in core/mime.rs must resolve to the same extractor.
    let aliased = registry
        .get("application/wordperfect")
        .expect("application/wordperfect (declared as an alias in core/mime.rs) must resolve to an extractor, not UnsupportedFormat");
    assert_eq!(aliased.name(), "wordperfect-extractor");
}

/// `application/x-quarto` is declared as an alias of `text/x-quarto` in
/// `core/mime.rs`; `MarkdownExtractor::supported_mime_types()` now registers
/// both, so the alias routes to the same extractor as the canonical string.
#[test]
fn quarto_alias_mime_resolves_to_markdown_extractor() {
    use xberg::extractors::ensure_initialized;
    use xberg::plugins::registry::get_document_extractor_registry;

    ensure_initialized().expect("failed to initialize default extractors");

    let registry = get_document_extractor_registry();
    let registry = registry.read();

    let canonical = registry
        .get("text/x-quarto")
        .expect("canonical Quarto MIME type must resolve to an extractor, not UnsupportedFormat");
    assert_eq!(canonical.name(), "markdown-extractor");

    let aliased = registry
        .get("application/x-quarto")
        .expect("application/x-quarto (declared as an alias in core/mime.rs) must resolve to an extractor, not UnsupportedFormat");
    assert_eq!(aliased.name(), "markdown-extractor");
}
