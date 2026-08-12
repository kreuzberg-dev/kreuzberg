//! Formula capture for XML formats.
//!
//! JATS and DocBook both wrap an equation in an element that may hold verbatim
//! TeX, a MathML subtree, or plain text. The capture below reads one such
//! element and returns its LaTeX, so each format states only the names of its
//! own child elements.

use crate::Result;
use crate::extraction::derive::strip_math_delimiters;
use crate::extractors::security::SecurityBudget;
use crate::text::utf8_validation;
use crate::utils::xml_utils::EntityReader;
use quick_xml::events::{BytesStart, Event};

/// The child elements a format uses inside a formula.
pub(crate) struct FormulaElements<'a> {
    /// Element whose text is verbatim TeX and wins over MathML. JATS names it
    /// `tex-math`; DocBook names it `alt`.
    pub(crate) tex: &'a str,
    /// Element whose text is the equation number, which becomes a LaTeX `\tag`.
    /// `None` for a format that has no such element.
    pub(crate) label: Option<&'a str>,
}

/// Return the local part of a possibly prefixed XML qualified name.
fn local_name_of(qname: &[u8]) -> String {
    let name = String::from_utf8_lossy(qname);
    match name.rsplit(':').next() {
        Some(local) => local.to_string(),
        None => name.into_owned(),
    }
}

/// Append a start tag to `buf` with the prefix stripped and namespace
/// declarations dropped, so the captured subtree parses without the
/// document's namespace context.
///
/// Attribute values are decoded and re-escaped, so single-quoted source
/// attributes with embedded double quotes stay well-formed. When two
/// attribute names collide after prefix stripping, the first one wins:
/// a duplicate attribute would make the captured XML unparseable.
fn write_start_tag(buf: &mut String, event: &BytesStart<'_>, self_closing: bool) {
    buf.push('<');
    buf.push_str(&local_name_of(event.name().as_ref()));
    let mut written: Vec<String> = Vec::new();
    for attr in event.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref());
        if key == "xmlns" || key.starts_with("xmlns:") {
            continue;
        }
        let local_key = key.rsplit(':').next().unwrap_or(&key).to_string();
        if written.contains(&local_key) {
            continue;
        }
        let raw = String::from_utf8_lossy(&attr.value);
        buf.push(' ');
        buf.push_str(&local_key);
        buf.push_str("=\"");
        match quick_xml::escape::unescape(&raw) {
            // Decoded value: re-escape for the double-quoted attribute.
            Ok(value) => buf.push_str(&quick_xml::escape::escape(value.as_ref())),
            // Undecodable reference: the raw bytes are already escaped, but a
            // literal quote from a single-quoted source attribute is not.
            Err(_) => buf.push_str(&raw.replace('"', "&quot;")),
        }
        buf.push('"');
        written.push(local_key);
    }
    if self_closing {
        buf.push('/');
    }
    buf.push('>');
}

/// Strip a full LaTeX document wrapper from verbatim TeX content.
///
/// PMC articles commonly ship `<tex-math>` as a complete compilable document
/// (`\documentclass...\begin{document}$$...$$\end{document}`); only the body
/// is the formula.
fn strip_latex_document_wrapper(tex: &str) -> &str {
    if !tex.contains("\\documentclass") {
        return tex;
    }
    let Some(start) = tex.find("\\begin{document}") else {
        return tex;
    };
    let body = &tex[start + "\\begin{document}".len()..];
    let body = match body.find("\\end{document}") {
        Some(end) => &body[..end],
        None => body,
    };
    body.trim()
}

/// Extract the LaTeX for a formula subtree.
///
/// The caller has consumed the formula start tag. The preference order is:
/// the TeX element's text verbatim, then the `math` subtree converted with the
/// shared MathML converter, then the flattened text content.
pub(crate) fn extract_formula_latex(
    reader: &mut EntityReader<'_>,
    budget: &mut SecurityBudget,
    names: &FormulaElements<'_>,
) -> Result<String> {
    let mut fallback_text = String::new();
    let mut tex_math = String::new();
    let mut label = String::new();
    let mut mathml_xmls: Vec<String> = Vec::new();
    let mut capture: Option<String> = None;
    let mut capture_depth = 0usize;
    let mut capture_in_alternatives = false;
    let mut alternatives_depth = 0usize;
    let mut alternatives_math_seen = false;
    let mut in_tex_math = false;
    let mut in_label = false;
    let mut depth = 0usize;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(s)) => {
                budget.enter()?;
                depth += 1;
                let local = local_name_of(s.name().as_ref());
                if let Some(buf) = capture.as_mut() {
                    capture_depth += 1;
                    let before = buf.len();
                    write_start_tag(buf, &s, false);
                    budget.account_text(buf.len() - before)?;
                } else if local == "math" {
                    let mut buf = String::new();
                    write_start_tag(&mut buf, &s, false);
                    budget.account_text(buf.len())?;
                    capture = Some(buf);
                    capture_depth = 1;
                    capture_in_alternatives = alternatives_depth > 0;
                } else if local == "alternatives" {
                    alternatives_depth += 1;
                } else if local == names.tex {
                    in_tex_math = true;
                } else if names.label.is_some_and(|name| local == name) {
                    in_label = true;
                }
            }
            Ok(Event::Empty(s)) => {
                if let Some(buf) = capture.as_mut() {
                    let before = buf.len();
                    write_start_tag(buf, &s, true);
                    budget.account_text(buf.len() - before)?;
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                if let Some(buf) = capture.as_mut() {
                    buf.push_str("</");
                    buf.push_str(&local_name_of(e.name().as_ref()));
                    buf.push('>');
                    capture_depth -= 1;
                    if capture_depth == 0
                        && let Some(xml) = capture.take()
                    {
                        // Inside `<alternatives>` every `math` sibling is one
                        // more representation of the SAME formula: keep the
                        // first. Outside, each sibling is its own equation.
                        if capture_in_alternatives {
                            if !alternatives_math_seen {
                                alternatives_math_seen = true;
                                mathml_xmls.push(xml);
                            }
                        } else {
                            mathml_xmls.push(xml);
                        }
                    }
                } else {
                    let local = local_name_of(e.name().as_ref());
                    if local == names.tex {
                        in_tex_math = false;
                    } else if names.label.is_some_and(|name| local == name) {
                        in_label = false;
                    } else if local == "alternatives" {
                        alternatives_depth = alternatives_depth.saturating_sub(1);
                    }
                }
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref()).to_string();
                if decoded.trim().is_empty() {
                    continue;
                }
                budget.check_entity(&decoded)?;
                budget.account_text(decoded.len())?;
                if let Some(buf) = capture.as_mut() {
                    buf.push_str(&quick_xml::escape::escape(&decoded));
                } else if in_tex_math {
                    tex_math.push_str(&decoded);
                } else if in_label {
                    if !label.is_empty() {
                        label.push(' ');
                    }
                    label.push_str(&decoded);
                } else {
                    fallback_text.push_str(&decoded);
                    fallback_text.push(' ');
                }
            }
            Ok(Event::CData(t)) => {
                let decoded = utf8_validation::from_utf8(t.as_ref()).unwrap_or("").to_string();
                if decoded.trim().is_empty() {
                    continue;
                }
                budget.check_entity(&decoded)?;
                budget.account_text(decoded.len())?;
                if in_tex_math {
                    tex_math.push_str(&decoded);
                } else if let Some(buf) = capture.as_mut() {
                    buf.push_str(&quick_xml::escape::escape(&decoded));
                } else {
                    fallback_text.push_str(&decoded);
                    fallback_text.push(' ');
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    // An equation label (`<label>1.1</label>`) becomes a LaTeX `\tag` so the
    // equation number survives the conversion. `\tag` renders inside parens,
    // so a source label that already carries them (`(1)`) sheds one pair —
    // otherwise the equation number displays as `((1))`.
    let with_tag = |latex: &str| -> String {
        let mut label = label.trim();
        if label.len() >= 2 && label.starts_with('(') && label.ends_with(')') {
            label = label[1..label.len() - 1].trim();
        }
        let label: String = label.chars().filter(|c| *c != '{' && *c != '}').collect();
        if label.is_empty() {
            latex.to_string()
        } else {
            format!("{latex} \\tag{{{label}}}")
        }
    };

    let tex = strip_math_delimiters(strip_latex_document_wrapper(tex_math.trim()));
    if !tex.is_empty() {
        return Ok(with_tag(tex));
    }
    if !mathml_xmls.is_empty() {
        let mut parts: Vec<String> = Vec::new();
        for xml in &mathml_xmls {
            let latex = crate::extraction::mathml::convert_mathml_str_to_latex(xml, budget)?;
            if !latex.trim().is_empty() {
                parts.push(latex.trim().to_string());
            }
        }
        if !parts.is_empty() {
            return Ok(with_tag(&parts.join(" \\\\ ")));
        }
    }
    if !label.trim().is_empty() {
        fallback_text = format!("{} {}", label.trim(), fallback_text);
    }
    Ok(fallback_text.trim().to_string())
}
