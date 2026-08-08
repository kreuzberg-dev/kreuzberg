//! Regression test for defect #69 — styled HTML emitted an unclosed `<section>` per slide.
//!
//! `rendering/html_styled.rs` wrote an opening `<section class="kb-slide" …>` for every
//! `ElementKind::Slide` but never wrote a closing tag and never rendered `elem.text`
//! (the slide title). `Slide` is a marker, not a container — there is no `SlideEnd`
//! kind — so every presentation rendered to styled HTML produced malformed markup and
//! silently lost its slide titles. The djot and comrak renderers both emit a break
//! followed by the title as a level-2 heading; styled HTML now matches that.
//!
//! Run with:
//!   cargo test -p xberg --features office --test issue_69_styled_html_slide_section

// `HtmlOutputConfig`, `HtmlTheme` and `ExtractionConfig::html_output` are all gated on the
// `html` feature, so without this guard the whole test target fails to compile under the
// default feature set and takes `cargo clippy --all-targets` down with it.
#![cfg(feature = "html")]

mod helpers;
use helpers::{extract_uri_document_blocking, get_test_documents_dir};

use xberg::core::config::{ExtractionConfig, HtmlOutputConfig, HtmlTheme, OutputFormat};

/// Extract a fixture as styled HTML with structural markup only (no embedded CSS),
/// so assertions see the element tree and nothing else.
fn styled_html(fixture: &str) -> String {
    let path = get_test_documents_dir().join(fixture);
    assert!(path.exists(), "fixture missing: {}", path.display());

    let config = ExtractionConfig {
        output_format: OutputFormat::Html,
        html_output: Some(HtmlOutputConfig {
            theme: HtmlTheme::Unstyled,
            embed_css: false,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    };

    let result = extract_uri_document_blocking(path, None, &config).expect("extraction must succeed");
    result.formatted_content.unwrap_or(result.content)
}

/// The fixture has two slides (`page1`, `page2`) with two paragraphs between them,
/// which pins down title rendering, tag balance, and sibling ordering at once.
#[test]
fn should_close_slide_section_and_render_slide_title() {
    let html = styled_html("odp/libreoffice_impress.odp");

    assert_eq!(
        html,
        concat!(
            r#"<div class="kb-doc"><main class="kb-content">"#,
            r#"<section class="kb-slide" data-slide="1"><h2 class="kb-h kb-h2">page1</h2></section>"#,
            r#"<p class="kb-p">This is</p>"#,
            r#"<p class="kb-p">An example Impress file</p>"#,
            r#"<section class="kb-slide" data-slide="2"><h2 class="kb-h kb-h2">page2</h2></section>"#,
            r#"</main></div>"#,
        ),
        "each slide must render its title and close its own section"
    );
}

/// Guards the structural invariant directly: no `<section>` may be left open.
/// Before the fix these counts were 2 and 0.
#[test]
fn should_emit_balanced_section_tags_for_every_presentation_fixture() {
    for fixture in [
        "odp/simple_text.odp",
        "odp/libreoffice_impress.odp",
        "odp/with_table.odp",
        "odp/with_images.odp",
    ] {
        let html = styled_html(fixture);
        let opened = html.matches(r#"<section class="kb-slide""#).count();
        let closed = html.matches("</section>").count();

        assert!(opened > 0, "{fixture}: expected at least one slide section, got none");
        assert_eq!(
            opened, closed,
            "{fixture}: {opened} <section> tags opened but {closed} closed — output is malformed"
        );
    }
}

/// The slide section must not swallow the content that follows it.
#[test]
fn should_render_post_slide_content_as_a_sibling_not_a_child() {
    let html = styled_html("odp/libreoffice_impress.odp");

    let first_section_end = html.find("</section>").expect("first slide section must be closed");
    let first_paragraph = html.find(r#"<p class="kb-p">This is</p>"#).expect("paragraph missing");

    assert!(
        first_section_end < first_paragraph,
        "slide section must close before the paragraph that follows it, got: {html}"
    );
}
