//! Render an `InternalDocument` to HTML5 via comrak.

use comrak::{Arena, format_html};

use crate::types::annotations::PdfAnnotation;
use crate::types::internal::InternalDocument;

use super::common::{annotation_display_text, annotation_type_label, escape_html_text};
use super::comrak_bridge::build_comrak_ast;
use super::markdown::comrak_options;

/// Render an `InternalDocument` to HTML5.
pub(crate) fn render_html(doc: &InternalDocument) -> String {
    let arena = Arena::new();
    let root = build_comrak_ast(doc, &arena);

    let mut options = comrak_options();
    options.render.r#unsafe = true;
    options.render.github_pre_lang = true;
    options.render.full_info_string = true;

    let mut output = String::new();
    format_html(root, &options, &mut output).expect("comrak HTML formatting should not fail");

    if let Some(annotations) = doc.annotations.as_deref() {
        output.push_str(&render_annotations_html(annotations));
    }

    output
}

/// Render the document-level PDF annotations (issue #63) as an HTML section:
/// an `<h2>` heading followed by a `<ul>` list, one `<li>` per annotation.
///
/// Returns an empty string when `annotations` is empty.
fn render_annotations_html(annotations: &[PdfAnnotation]) -> String {
    if annotations.is_empty() {
        return String::new();
    }

    let mut out = String::from("<h2>Annotations</h2>\n<ul>\n");
    for annotation in annotations {
        out.push_str("<li><strong>");
        out.push_str(annotation_type_label(annotation.annotation_type));
        out.push_str("</strong> (page ");
        out.push_str(&annotation.page_number.to_string());
        out.push(')');

        if let Some(author) = annotation.author.as_deref().filter(|s| !s.is_empty()) {
            out.push_str(" by ");
            out.push_str(&escape_html_text(author));
        }

        if let Some(text) = annotation_display_text(annotation) {
            out.push_str(": ");
            out.push_str(&escape_html_text(text));
        }

        out.push_str("</li>\n");
    }
    out.push_str("</ul>\n");
    out
}
