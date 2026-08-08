//! Styled HTML renderer — direct `InternalDocument` → HTML5 with `kb-*` class hooks.
//!
//! Replaces the comrak-based `render_html` when the `html` feature is
//! active and `ExtractionConfig::html_output` is `Some(...)`.
//!
//! # Stability contract
//!
//! All emitted class names (prefixed by `config.class_prefix`, default `"kb-"`)
//! and CSS custom properties (`--kb-*`) are stable across minor versions.
//! See `docs/reference/html-styling-contract.md`.

use std::fmt::Write as FmtWrite;
use std::sync::Arc;

use v_htmlescape::escape_string;

use crate::Result;
use crate::XbergError;
use crate::core::config::html_output::{HtmlOutputConfig, HtmlTheme};
use crate::plugins::{InternalRenderer, Plugin};
use crate::rendering::common::{NestingKind, RenderState, render_annotated_text_with_plain};
use crate::types::document_structure::{AnnotationKind, ContentLayer};
use crate::types::internal::{ElementKind, InternalDocument};

fn theme_css(theme: &HtmlTheme) -> &'static str {
    match theme {
        HtmlTheme::Unstyled => "",
        HtmlTheme::Default | HtmlTheme::Light => DEFAULT_CSS,
        HtmlTheme::GitHub => GITHUB_CSS,
        HtmlTheme::Dark => DARK_CSS,
    }
}

const DEFAULT_CSS: &str = r#":root {
  --kb-font-family: system-ui, sans-serif;
  --kb-mono-font-family: ui-monospace, monospace;
  --kb-text-color: #1a1a1a;
  --kb-bg-color: #ffffff;
  --kb-heading-color: #111111;
  --kb-link-color: #0066cc;
  --kb-link-hover-color: #004499;
  --kb-code-bg: #f5f5f5;
  --kb-code-color: #c7254e;
  --kb-border-color: #e0e0e0;
  --kb-table-border: #cccccc;
  --kb-blockquote-border: #0066cc;
  --kb-max-width: 72ch;
  --kb-line-height: 1.6;
}
.kb-doc { font-family: var(--kb-font-family); color: var(--kb-text-color); background: var(--kb-bg-color); line-height: var(--kb-line-height); }
.kb-content { max-width: var(--kb-max-width); margin: 0 auto; padding: 1rem; }
.kb-h { color: var(--kb-heading-color); margin: 1.5em 0 0.5em; line-height: 1.25; }
.kb-p { margin: 0.75em 0; }
.kb-list { margin: 0.75em 0; padding-left: 2em; }
.kb-li { margin: 0.25em 0; }
.kb-blockquote { border-left: 4px solid var(--kb-blockquote-border); margin: 1em 0; padding: 0.5em 1em; color: #555; }
.kb-pre { background: var(--kb-code-bg); border: 1px solid var(--kb-border-color); border-radius: 4px; overflow-x: auto; padding: 1em; margin: 1em 0; }
.kb-code { font-family: var(--kb-mono-font-family); font-size: 0.875em; }
p .kb-code { background: var(--kb-code-bg); color: var(--kb-code-color); padding: 0.1em 0.3em; border-radius: 3px; }
.kb-table { border-collapse: collapse; width: 100%; margin: 1em 0; }
.kb-th, .kb-td { border: 1px solid var(--kb-table-border); padding: 0.5em 0.75em; text-align: left; }
.kb-th { background: var(--kb-code-bg); font-weight: 600; }
.kb-figure { margin: 1em 0; }
.kb-img { max-width: 100%; height: auto; }
.kb-page-break { border: none; border-top: 1px dashed var(--kb-border-color); margin: 2em 0; }
"#;

const GITHUB_CSS: &str = r#":root {
  --kb-font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Helvetica, Arial, sans-serif;
  --kb-mono-font-family: SFMono-Regular, Consolas, "Liberation Mono", Menlo, monospace;
  --kb-text-color: #24292f;
  --kb-bg-color: #ffffff;
  --kb-heading-color: #24292f;
  --kb-link-color: #0969da;
  --kb-link-hover-color: #0550ae;
  --kb-code-bg: #f6f8fa;
  --kb-code-color: #e01e5a;
  --kb-border-color: #d0d7de;
  --kb-table-border: #d0d7de;
  --kb-blockquote-border: #d0d7de;
  --kb-max-width: 80ch;
  --kb-line-height: 1.5;
}
.kb-doc { font-family: var(--kb-font-family); color: var(--kb-text-color); background: var(--kb-bg-color); line-height: var(--kb-line-height); }
.kb-content { max-width: var(--kb-max-width); margin: 0 auto; padding: 1rem 2rem; }
.kb-h { color: var(--kb-heading-color); margin: 1.5em 0 0.5em; border-bottom: 1px solid var(--kb-border-color); padding-bottom: 0.3em; }
.kb-p { margin: 0.75em 0; }
.kb-list { margin: 0.75em 0; padding-left: 2em; }
.kb-li { margin: 0.25em 0; }
.kb-blockquote { border-left: 4px solid var(--kb-blockquote-border); margin: 1em 0; padding: 0.5em 1em; color: #57606a; }
.kb-pre { background: var(--kb-code-bg); border: 1px solid var(--kb-border-color); border-radius: 6px; overflow-x: auto; padding: 1em; margin: 1em 0; }
.kb-code { font-family: var(--kb-mono-font-family); font-size: 85%; }
p .kb-code { background: var(--kb-code-bg); color: var(--kb-code-color); padding: 0.2em 0.4em; border-radius: 6px; }
.kb-table { border-collapse: collapse; width: 100%; margin: 1em 0; }
.kb-th, .kb-td { border: 1px solid var(--kb-table-border); padding: 0.4em 0.8em; }
.kb-th { background: var(--kb-code-bg); font-weight: 600; }
.kb-figure { margin: 1em 0; }
.kb-img { max-width: 100%; height: auto; }
.kb-page-break { border: none; border-top: 1px dashed var(--kb-border-color); margin: 2em 0; }
"#;

const DARK_CSS: &str = r#":root {
  --kb-font-family: system-ui, sans-serif;
  --kb-mono-font-family: ui-monospace, monospace;
  --kb-text-color: #e6edf3;
  --kb-bg-color: #0d1117;
  --kb-heading-color: #f0f6fc;
  --kb-link-color: #58a6ff;
  --kb-link-hover-color: #79c0ff;
  --kb-code-bg: #161b22;
  --kb-code-color: #ff7b72;
  --kb-border-color: #30363d;
  --kb-table-border: #30363d;
  --kb-blockquote-border: #3d444d;
  --kb-max-width: 72ch;
  --kb-line-height: 1.6;
}
.kb-doc { font-family: var(--kb-font-family); color: var(--kb-text-color); background: var(--kb-bg-color); line-height: var(--kb-line-height); }
.kb-content { max-width: var(--kb-max-width); margin: 0 auto; padding: 1rem; }
.kb-h { color: var(--kb-heading-color); margin: 1.5em 0 0.5em; }
.kb-p { margin: 0.75em 0; }
.kb-list { margin: 0.75em 0; padding-left: 2em; }
.kb-li { margin: 0.25em 0; }
.kb-blockquote { border-left: 4px solid var(--kb-blockquote-border); margin: 1em 0; padding: 0.5em 1em; color: #8d96a0; }
.kb-pre { background: var(--kb-code-bg); border: 1px solid var(--kb-border-color); border-radius: 4px; overflow-x: auto; padding: 1em; margin: 1em 0; }
.kb-code { font-family: var(--kb-mono-font-family); font-size: 0.875em; }
p .kb-code { background: var(--kb-code-bg); color: var(--kb-code-color); padding: 0.1em 0.3em; border-radius: 3px; }
.kb-table { border-collapse: collapse; width: 100%; margin: 1em 0; }
.kb-th, .kb-td { border: 1px solid var(--kb-table-border); padding: 0.5em 0.75em; }
.kb-th { background: var(--kb-code-bg); font-weight: 600; }
.kb-figure { margin: 1em 0; }
.kb-img { max-width: 100%; height: auto; }
.kb-page-break { border: none; border-top: 1px dashed var(--kb-border-color); margin: 2em 0; }
"#;

/// Styled HTML renderer.
///
/// Implements the [`crate::plugins::Renderer`] trait; registered as `"html"` when the
/// `html` feature is active. Configuration is baked in at
/// construction time — no per-render allocation for CSS resolution.
#[cfg_attr(alef, alef(skip))]
pub struct StyledHtmlRenderer {
    config: Arc<HtmlOutputConfig>,
    /// Resolved CSS: theme + css_file + inline css, concatenated once.
    resolved_css: String,
}

impl StyledHtmlRenderer {
    /// Build a renderer from the given configuration.
    ///
    /// Loads `css_file` from disk and concatenates all CSS sources. Returns
    /// an error if `css_file` is set but cannot be read.
    /// Maximum size in bytes for a CSS file loaded via `css_file`.
    const MAX_CSS_FILE_SIZE: u64 = 1_048_576;

    pub(crate) fn new(config: HtmlOutputConfig) -> Result<Self> {
        if !config
            .class_prefix
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
        {
            return Err(XbergError::validation(format!(
                "html_output.class_prefix must contain only alphanumerics, hyphens, and underscores, got: {:?}",
                config.class_prefix
            )));
        }

        if config.class_prefix != "kb-" && config.theme != HtmlTheme::Unstyled {
            tracing::warn!(
                "html_output.class_prefix is {:?} but theme is {:?}; \
                 built-in theme CSS targets `.kb-*` classes and will not match. \
                 Use theme: \"unstyled\" with a custom prefix.",
                config.class_prefix,
                config.theme,
            );
        }

        let mut css = theme_css(&config.theme).to_owned();

        if let Some(ref path) = config.css_file {
            let metadata = std::fs::metadata(path).map_err(|e| XbergError::Parsing {
                message: format!("html_output.css_file \"{}\": {}", path.display(), e),
                source: None,
            })?;
            if metadata.len() > Self::MAX_CSS_FILE_SIZE {
                return Err(XbergError::validation(format!(
                    "html_output.css_file \"{}\": file size {} exceeds maximum of {} bytes",
                    path.display(),
                    metadata.len(),
                    Self::MAX_CSS_FILE_SIZE,
                )));
            }
            let file_css = std::fs::read_to_string(path).map_err(|e| XbergError::Parsing {
                message: format!("html_output.css_file \"{}\": {}", path.display(), e),
                source: None,
            })?;
            css.push('\n');
            css.push_str(&file_css);
        }

        if let Some(ref inline) = config.css {
            css.push('\n');
            css.push_str(inline);
        }

        let css = css.replace("</style>", "").replace("</STYLE>", "");

        Ok(Self {
            config: Arc::new(config),
            resolved_css: css,
        })
    }
}

impl Plugin for StyledHtmlRenderer {
    fn name(&self) -> &str {
        "html"
    }
}

impl InternalRenderer for StyledHtmlRenderer {
    fn render(&self, doc: &InternalDocument) -> Result<String> {
        let p = &self.config.class_prefix;

        let text_bytes: usize = doc.elements.iter().map(|e| e.text.len()).sum();
        let cap = (text_bytes * 3).min(64 * 1024 * 1024);
        let mut buf = String::with_capacity(cap);

        write!(buf, r#"<div class="{p}doc">"#).unwrap();

        if self.config.embed_css && !self.resolved_css.is_empty() {
            buf.push_str("<style>");
            buf.push_str(&self.resolved_css);
            buf.push_str("</style>");
        }

        write!(buf, r#"<main class="{p}content">"#).unwrap();
        render_elements(doc, p, &mut buf);
        buf.push_str("</main></div>");

        Ok(buf)
    }
}

fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    escape_string(text, &mut out);
    out
}

fn render_elements(doc: &InternalDocument, p: &str, buf: &mut String) {
    let mut state = RenderState::default();
    let mut list_ordered_stack: Vec<bool> = Vec::new();

    for elem in &doc.elements {
        if !matches!(elem.layer, ContentLayer::Body) {
            continue;
        }

        match elem.kind {
            ElementKind::Title => {
                write!(buf, r#"<h1 class="{p}doc-title">{}</h1>"#, esc(&elem.text)).unwrap();
            }
            ElementKind::Heading { level } => {
                let lvl = level.clamp(1, 6);
                write!(
                    buf,
                    r#"<h{lvl} class="{p}h {p}h{lvl}">{}</h{lvl}>"#,
                    render_inline(doc, elem, p)
                )
                .unwrap();
            }

            ElementKind::Paragraph | ElementKind::OcrText { .. } => {
                write!(buf, r#"<p class="{p}p">{}</p>"#, render_inline(doc, elem, p)).unwrap();
            }

            ElementKind::ListStart { ordered } => {
                list_ordered_stack.push(ordered);
                state.push_container(NestingKind::List { ordered, item_count: 0 }, elem.depth);
                if ordered {
                    write!(buf, r#"<ol class="{p}list {p}ol">"#).unwrap();
                } else {
                    write!(buf, r#"<ul class="{p}list {p}ul">"#).unwrap();
                }
            }
            ElementKind::ListEnd => {
                let ordered = list_ordered_stack.pop().unwrap_or(false);
                state.pop_container(&NestingKind::List { ordered, item_count: 0 });
                if ordered {
                    buf.push_str("</ol>");
                } else {
                    buf.push_str("</ul>");
                }
            }
            ElementKind::ListItem { .. } => {
                write!(buf, r#"<li class="{p}li">{}</li>"#, render_inline(doc, elem, p)).unwrap();
            }

            ElementKind::QuoteStart => {
                state.push_container(NestingKind::BlockQuote, elem.depth);
                write!(buf, r#"<blockquote class="{p}blockquote">"#).unwrap();
            }
            ElementKind::QuoteEnd => {
                state.pop_container(&NestingKind::BlockQuote);
                buf.push_str("</blockquote>");
            }

            ElementKind::Code => {
                let lang = elem
                    .attributes
                    .as_ref()
                    .and_then(|a| a.get("language").or_else(|| a.get("lang")))
                    .map(|s| s.as_str())
                    .unwrap_or("");
                if lang.is_empty() {
                    write!(
                        buf,
                        r#"<pre class="{p}pre"><code class="{p}code">{}</code></pre>"#,
                        esc(&elem.text)
                    )
                    .unwrap();
                } else {
                    let safe_lang = esc(lang);
                    write!(
                        buf,
                        r#"<pre class="{p}pre"><code class="{p}code {p}lang-{safe_lang}">{}</code></pre>"#,
                        esc(&elem.text)
                    )
                    .unwrap();
                }
            }

            ElementKind::Formula => {
                // Math, not code: emit LaTeX in `$$...$$` display-math delimiters inside a
                // math-classed element, matching the `dollar_math`/`display_math` convention
                // used by the comrak and djot renderers. KaTeX/MathJax auto-render both pick
                // up `$$...$$` by default, so this is renderable as-is once such a script is
                // present on the page; without one it degrades to visible LaTeX source rather
                // than a misleading monospace code block.
                write!(
                    buf,
                    r#"<div class="{p}formula {p}math" data-math-style="display">$${}$$</div>"#,
                    esc(&elem.text)
                )
                .unwrap();
            }

            ElementKind::FootnoteDefinition => {
                let anchor = elem.anchor.as_deref().unwrap_or("");
                write!(
                    buf,
                    r#"<aside class="{p}footnote" id="fn-{}">{}</aside>"#,
                    esc(anchor),
                    render_inline(doc, elem, p)
                )
                .unwrap();
            }
            ElementKind::FootnoteRef => {
                let anchor = elem.anchor.as_deref().unwrap_or("");
                write!(
                    buf,
                    r##"<sup class="{p}footnote-ref"><a href="#fn-{}">{}</a></sup>"##,
                    esc(anchor),
                    esc(&elem.text)
                )
                .unwrap();
            }
            ElementKind::CommentDefinition => {
                let anchor = elem.anchor.as_deref().unwrap_or("");
                write!(
                    buf,
                    r#"<aside class="{p}comment" id="fn-{}">{}</aside>"#,
                    esc(anchor),
                    render_inline(doc, elem, p)
                )
                .unwrap();
            }
            ElementKind::CommentRef => {
                let anchor = elem.anchor.as_deref().unwrap_or("");
                write!(
                    buf,
                    r##"<sup class="{p}comment-ref"><a href="#fn-{}">{}</a></sup>"##,
                    esc(anchor),
                    esc(&elem.text)
                )
                .unwrap();
            }
            ElementKind::Citation => {
                write!(buf, r#"<cite class="{p}citation">{}</cite>"#, esc(&elem.text)).unwrap();
            }

            ElementKind::Slide { number } => {
                // `Slide` is a marker, not a container: there is no `SlideEnd` element kind, so
                // an unbalanced opening tag would leave every slide deck's HTML malformed. Match
                // the djot and comrak renderers, which emit a break followed by the slide title
                // as a level-2 heading, and keep the `slide`/`data-slide` hooks for styling.
                write!(buf, r#"<section class="{p}slide" data-slide="{number}">"#).unwrap();
                if !elem.text.is_empty() {
                    write!(buf, r#"<h2 class="{p}h {p}h2">{}</h2>"#, render_inline(doc, elem, p)).unwrap();
                }
                buf.push_str("</section>");
            }

            ElementKind::DefinitionTerm => {
                write!(buf, r#"<dt class="{p}dt">{}</dt>"#, render_inline(doc, elem, p)).unwrap();
            }
            ElementKind::DefinitionDescription => {
                write!(buf, r#"<dd class="{p}dd">{}</dd>"#, render_inline(doc, elem, p)).unwrap();
            }

            ElementKind::Admonition => {
                let kind = elem
                    .attributes
                    .as_ref()
                    .and_then(|a| a.get("kind").or_else(|| a.get("type")))
                    .map(|s| s.as_str())
                    .unwrap_or("note");
                let safe_kind = esc(kind);
                write!(
                    buf,
                    r#"<aside class="{p}admonition {p}admonition-{safe_kind}">{}</aside>"#,
                    render_inline(doc, elem, p)
                )
                .unwrap();
            }

            ElementKind::RawBlock => {
                buf.push_str(&elem.text);
            }
            ElementKind::MetadataBlock => {
                write!(buf, r#"<dl class="{p}metadata">{}</dl>"#, esc(&elem.text)).unwrap();
            }

            ElementKind::GroupStart => {
                state.push_container(NestingKind::Group, elem.depth);
                write!(buf, r#"<div class="{p}group">"#).unwrap();
            }
            ElementKind::GroupEnd => {
                state.pop_container(&NestingKind::Group);
                buf.push_str("</div>");
            }

            ElementKind::Table { table_index } => {
                if let Some(table) = doc.tables.get(table_index as usize) {
                    render_table(table, p, buf);
                }
            }

            ElementKind::Image { image_index } => {
                if let Some(image) = doc.images.get(image_index as usize) {
                    let render_ocr = elem.should_render_image_ocr();
                    render_image(
                        image,
                        &elem.text,
                        p,
                        doc.ocr_text_only && render_ocr,
                        doc.append_ocr_text && render_ocr,
                        buf,
                    );
                }
            }

            ElementKind::PageBreak => {
                let page = elem.page.unwrap_or(0);
                write!(buf, r#"<hr class="{p}page-break" data-page="{page}">"#).unwrap();
            }
        }
    }
}

fn render_inline(_doc: &InternalDocument, elem: &crate::types::internal::InternalElement, p: &str) -> String {
    if elem.annotations.is_empty() {
        return esc(&elem.text);
    }

    let p = p.to_string();
    render_annotated_text_with_plain(
        &elem.text,
        &elem.annotations,
        move |span, kind| match kind {
            AnnotationKind::Bold => format!("<strong>{}</strong>", esc(span)),
            AnnotationKind::Italic => format!("<em>{}</em>", esc(span)),
            AnnotationKind::Strikethrough => format!("<del>{}</del>", esc(span)),
            AnnotationKind::Link { url, .. } => {
                format!(r#"<a class="{p}link" href="{}">{}</a>"#, esc(url), esc(span))
            }
            _ => esc(span),
        },
        esc,
    )
}

fn render_table(table: &crate::types::tables::Table, p: &str, buf: &mut String) {
    write!(buf, r#"<table class="{p}table">"#).unwrap();
    let mut rows = table.cells.iter().peekable();

    if let Some(header_row) = rows.next() {
        write!(buf, r#"<thead class="{p}thead"><tr class="{p}tr">"#).unwrap();
        for cell in header_row {
            write!(buf, r#"<th class="{p}th">{}</th>"#, esc(cell)).unwrap();
        }
        buf.push_str("</tr></thead>");
    }

    if rows.peek().is_some() {
        write!(buf, r#"<tbody class="{p}tbody">"#).unwrap();
        for row in rows {
            write!(buf, r#"<tr class="{p}tr">"#).unwrap();
            for cell in row {
                write!(buf, r#"<td class="{p}td">{}</td>"#, esc(cell)).unwrap();
            }
            buf.push_str("</tr>");
        }
        buf.push_str("</tbody>");
    }

    buf.push_str("</table>");
}

fn render_image(
    image: &crate::types::ExtractedImage,
    alt: &str,
    p: &str,
    ocr_text_only: bool,
    append_ocr_text: bool,
    buf: &mut String,
) {
    use base64::Engine as _;

    let ocr_content = image
        .ocr_result
        .as_ref()
        .map(|r| r.content.as_str())
        .filter(|s| !s.is_empty());

    if ocr_text_only && let Some(text) = ocr_content {
        write!(buf, r#"<p class="{p}ocr-text">{}</p>"#, esc(text)).unwrap();
        return;
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&image.data);
    let mime = match image.format.as_ref() {
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "tiff" => "image/tiff",
        _ => "image/png",
    };
    write!(
        buf,
        r#"<figure class="{p}figure"><img class="{p}img" src="data:{mime};base64,{b64}" alt="{}">"#,
        esc(alt)
    )
    .unwrap();

    if append_ocr_text && let Some(text) = ocr_content {
        write!(buf, r#"<figcaption class="{p}figcaption">{}</figcaption>"#, esc(text)).unwrap();
    }

    buf.push_str("</figure>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::html_output::HtmlOutputConfig;
    use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

    fn make_doc(kind: ElementKind, text: &str) -> InternalDocument {
        let mut doc = InternalDocument::new("text/plain");
        doc.push_element(InternalElement::text(kind, text, 0));
        doc
    }

    fn render(config: HtmlOutputConfig, doc: &InternalDocument) -> String {
        StyledHtmlRenderer::new(config).unwrap().render(doc).unwrap()
    }

    #[test]
    fn paragraph_emits_kb_p() {
        let doc = make_doc(ElementKind::Paragraph, "Hello");
        let out = render(HtmlOutputConfig::default(), &doc);
        assert!(out.contains(r#"class="kb-p""#), "missing kb-p: {out}");
        assert!(out.contains("Hello"), "text missing: {out}");
    }

    #[test]
    fn heading_emits_level_classes() {
        let doc = make_doc(ElementKind::Heading { level: 2 }, "Section");
        let out = render(HtmlOutputConfig::default(), &doc);
        assert!(out.contains(r#"class="kb-h kb-h2""#), "missing heading classes: {out}");
        assert!(out.contains("<h2"), "wrong tag: {out}");
    }

    #[test]
    fn wrapper_always_present() {
        let doc = InternalDocument::new("text/plain");
        let out = render(HtmlOutputConfig::default(), &doc);
        assert!(out.contains(r#"class="kb-doc""#));
        assert!(out.contains(r#"class="kb-content""#));
    }

    #[test]
    fn embed_css_true_includes_style_block() {
        let doc = InternalDocument::new("text/plain");
        let out = render(
            HtmlOutputConfig {
                embed_css: true,
                theme: HtmlTheme::Default,
                ..Default::default()
            },
            &doc,
        );
        assert!(out.contains("<style>"), "style block missing: {out}");
    }

    #[test]
    fn embed_css_false_omits_style_block() {
        let doc = InternalDocument::new("text/plain");
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(!out.contains("<style>"), "unexpected style block: {out}");
    }

    #[test]
    fn user_css_appears_in_style_block() {
        let doc = InternalDocument::new("text/plain");
        let cfg = HtmlOutputConfig {
            css: Some(".kb-p { color: red; }".to_string()),
            embed_css: true,
            ..Default::default()
        };
        let out = render(cfg, &doc);
        assert!(out.contains(".kb-p { color: red; }"), "user css missing: {out}");
    }

    #[test]
    fn custom_prefix_replaces_kb() {
        let doc = make_doc(ElementKind::Paragraph, "test");
        let cfg = HtmlOutputConfig {
            class_prefix: "my-".to_string(),
            embed_css: false,
            ..Default::default()
        };
        let out = render(cfg, &doc);
        assert!(out.contains(r#"class="my-doc""#), "custom prefix missing: {out}");
        assert!(out.contains(r#"class="my-p""#), "custom prefix on p missing: {out}");
        assert!(!out.contains("kb-"), "old prefix still present: {out}");
    }

    #[test]
    fn formula_emits_dollar_math_not_code_block() {
        let doc = make_doc(ElementKind::Formula, "x^2 + y^2 = z^2");
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(
            out.contains(r#"<div class="kb-formula kb-math" data-math-style="display">$$x^2 + y^2 = z^2$$</div>"#),
            "unexpected formula markup: {out}"
        );
        assert!(
            !out.contains("<pre"),
            "formula should not render as a code block: {out}"
        );
        assert!(
            !out.contains("<code"),
            "formula should not render as a code block: {out}"
        );
    }

    #[test]
    fn formula_escapes_html_special_characters() {
        let doc = make_doc(ElementKind::Formula, "a < b & b > c");
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(
            out.contains("$$a &lt; b &amp; b &gt; c$$"),
            "escaped formula missing: {out}"
        );
        assert!(!out.contains("a < b & b > c"), "raw unescaped formula leaked: {out}");
    }

    #[test]
    fn text_content_is_escaped() {
        let doc = make_doc(ElementKind::Paragraph, "<script>alert(1)</script>");
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(!out.contains("<script>"), "unescaped script tag: {out}");
        assert!(out.contains("&lt;script&gt;"), "escaped form missing: {out}");
    }

    #[test]
    fn list_items_wrapped_in_ul() {
        let mut doc = InternalDocument::new("text/plain");
        doc.push_element(InternalElement::text(ElementKind::ListStart { ordered: false }, "", 0));
        doc.push_element(InternalElement::text(
            ElementKind::ListItem { ordered: false },
            "Item",
            1,
        ));
        doc.push_element(InternalElement::text(ElementKind::ListEnd, "", 0));
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(out.contains(r#"class="kb-list kb-ul""#), "ul missing: {out}");
        assert!(out.contains(r#"class="kb-li""#), "li missing: {out}");
        assert!(out.contains("</ul>"), "closing ul missing: {out}");
    }

    #[test]
    fn table_emits_thead_tbody() {
        use crate::types::tables::Table;
        let mut doc = InternalDocument::new("text/plain");
        let table = Table {
            cells: vec![
                vec!["H1".to_string(), "H2".to_string()],
                vec!["A".to_string(), "B".to_string()],
            ],
            markdown: String::new(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        };
        let idx = doc.push_table(table);
        doc.push_element(InternalElement::text(ElementKind::Table { table_index: idx }, "", 0));
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(out.contains(r#"class="kb-table""#), "table class missing: {out}");
        assert!(out.contains(r#"class="kb-thead""#), "thead missing: {out}");
        assert!(out.contains(r#"class="kb-tbody""#), "tbody missing: {out}");
        assert!(out.contains("<th"), "th missing: {out}");
        assert!(out.contains("<td"), "td missing: {out}");
    }

    fn make_image_doc(ocr_text_only: bool, append_ocr_text: bool) -> InternalDocument {
        use crate::types::ExtractedImage;
        use std::borrow::Cow;
        let mut doc = InternalDocument::new("test");
        doc.ocr_text_only = ocr_text_only;
        doc.append_ocr_text = append_ocr_text;
        let idx = doc.push_image(ExtractedImage {
            data: bytes::Bytes::from_static(b"\x89PNG"),
            format: Cow::Borrowed("png"),
            image_index: 0,
            page_number: None,
            width: None,
            height: None,
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
            ocr_result: Some(Box::new(crate::types::ExtractedDocument {
                content: "OCR caption".to_string(),
                mime_type: Cow::Borrowed("text/plain"),
                ..Default::default()
            })),
            bounding_box: None,
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        });
        doc.push_element(crate::types::internal::InternalElement::text(
            crate::types::internal::ElementKind::Image { image_index: idx },
            "alt text",
            0,
        ));
        doc
    }

    #[test]
    fn image_no_flags_emits_figure_without_figcaption() {
        let doc = make_image_doc(false, false);
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(out.contains("<figure"), "expected <figure>: {out}");
        assert!(
            !out.contains("<figcaption"),
            "unexpected figcaption when both flags false: {out}"
        );
        assert!(
            !out.contains("OCR caption"),
            "unexpected OCR text when both flags false: {out}"
        );
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn image_internal_suppression_preserves_figure_without_nested_ocr() {
        let mut doc = make_image_doc(false, true);
        doc.elements[0].suppress_image_ocr_rendering();

        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );

        assert!(out.contains("<figure"));
        assert!(!out.contains("OCR caption"));
        assert!(doc.images[0].ocr_result.is_some());
    }

    #[test]
    fn image_append_ocr_text_emits_figcaption() {
        let doc = make_image_doc(false, true);
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(out.contains("<figure"), "expected <figure>: {out}");
        assert!(
            out.contains("<figcaption"),
            "expected figcaption with append_ocr_text=true: {out}"
        );
        assert!(out.contains("OCR caption"), "expected OCR text in figcaption: {out}");
    }

    #[test]
    fn image_ocr_text_only_emits_paragraph_not_figure() {
        let doc = make_image_doc(true, false);
        let out = render(
            HtmlOutputConfig {
                embed_css: false,
                ..Default::default()
            },
            &doc,
        );
        assert!(
            !out.contains("<figure"),
            "unexpected <figure> when ocr_text_only=true: {out}"
        );
        assert!(out.contains("OCR caption"), "expected OCR text as paragraph: {out}");
        assert!(out.contains("kb-ocr-text"), "expected ocr-text class: {out}");
    }
}
