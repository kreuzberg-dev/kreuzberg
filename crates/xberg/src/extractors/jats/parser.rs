//! XML parsing and document structure traversal for JATS documents.

use crate::Result;
use crate::extractors::security::SecurityBudget;
use crate::text::utf8_validation;
use quick_xml::events::{BytesStart, Event};

use crate::utils::xml_utils::EntityReader;

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

use crate::extraction::derive::strip_math_delimiters;

/// Extract the LaTeX for a `disp-formula` / `inline-formula` subtree.
///
/// The caller has consumed the formula start tag. The preference order is:
/// a `tex-math` child's text verbatim, then the `mml:math` subtree converted
/// with the shared MathML converter, then the flattened text content.
pub(super) fn extract_formula_latex(reader: &mut EntityReader<'_>, budget: &mut SecurityBudget) -> Result<String> {
    let mut fallback_text = String::new();
    let mut tex_math = String::new();
    let mut label = String::new();
    let mut mathml_xmls: Vec<String> = Vec::new();

    let mut capture: Option<String> = None;
    let mut capture_depth = 0usize;
    let mut capture_in_alternatives = false;
    let mut alternatives_math_seen = false;
    let mut in_tex_math = false;
    let mut in_label = false;
    let mut alternatives_depth = 0usize;
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
                } else if local == "tex-math" {
                    in_tex_math = true;
                } else if local == "label" {
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
                    match local_name_of(e.name().as_ref()).as_str() {
                        "tex-math" => in_tex_math = false,
                        "label" => in_label = false,
                        "alternatives" => alternatives_depth = alternatives_depth.saturating_sub(1),
                        _ => {}
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
    // equation number survives the conversion.
    let with_tag = |latex: &str| -> String {
        let label: String = label.trim().chars().filter(|c| *c != '{' && *c != '}').collect();
        if label.is_empty() {
            latex.to_string()
        } else {
            format!("{latex} \\tag{{{label}}}")
        }
    };

    let tex = strip_math_delimiters(tex_math.trim());
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

/// Extract text content from a JATS element and its children.
pub(super) fn extract_text_content(reader: &mut EntityReader<'_>, budget: &mut SecurityBudget) -> Result<String> {
    let mut text = String::new();
    let mut depth = 0;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(_)) => {
                budget.enter()?;
                depth += 1;
            }
            Ok(Event::End(_)) => {
                budget.leave();
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push(' ');
                }
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref()).to_string();
                if !decoded.trim().is_empty() {
                    budget.check_entity(&decoded)?;
                    budget.account_text(decoded.len())?;
                    text.push_str(&decoded);
                    text.push(' ');
                }
            }
            Ok(Event::CData(t)) => {
                let decoded = utf8_validation::from_utf8(t.as_ref()).unwrap_or("").to_string();
                if !decoded.trim().is_empty() {
                    budget.check_entity(&decoded)?;
                    budget.account_text(decoded.len())?;
                    text.push_str(&decoded);
                    text.push('\n');
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(text.trim().to_string())
}

/// Extract a formatted citation string from a `<ref>` element.
///
/// Parses structured `<element-citation>` children (person-group, article-title,
/// source, year, volume, fpage, lpage) into a conventional citation string like:
/// `Brown T, Davis K. Cognitive effects of caffeine. J Neurosci. 2002;15:234-241.`
///
/// Falls back to plain text extraction for `<mixed-citation>` or unrecognized structures.
pub(super) fn extract_citation_text(reader: &mut EntityReader<'_>, budget: &mut SecurityBudget) -> Result<String> {
    let mut depth: u32 = 0;
    let mut in_element_citation = false;
    let mut in_mixed_citation = false;
    let mut in_person_group = false;
    let mut in_name = false;

    let mut authors: Vec<String> = Vec::new();
    let mut current_surname = String::new();
    let mut current_given = String::new();
    let mut article_title = String::new();
    let mut source = String::new();
    let mut year = String::new();
    let mut volume = String::new();
    let mut fpage = String::new();
    let mut lpage = String::new();
    let mut doi = String::new();
    let mut publisher_name = String::new();
    let mut publisher_loc = String::new();

    let mut current_tag = String::new();

    let mut mixed_text = String::new();

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                depth += 1;
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag.as_str() {
                    "element-citation" => {
                        in_element_citation = true;
                    }
                    "mixed-citation" => {
                        in_mixed_citation = true;
                    }
                    "person-group" if in_element_citation => {
                        in_person_group = true;
                    }
                    "name" if in_person_group => {
                        in_name = true;
                        current_surname.clear();
                        current_given.clear();
                    }
                    "surname" | "given-names" | "article-title" | "source" | "year" | "volume" | "fpage" | "lpage"
                    | "publisher-name" | "publisher-loc"
                        if in_element_citation =>
                    {
                        current_tag = tag;
                    }
                    "pub-id" | "article-id" if in_element_citation => {
                        let mut id_type = String::new();
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "pub-id-type" {
                                id_type = val.to_string();
                            }
                        }
                        if id_type == "doi" {
                            current_tag = "pub-id-doi".to_string();
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                if depth == 0 {
                    break;
                }
                let tag = String::from_utf8_lossy(e.name().as_ref()).to_string();

                match tag.as_str() {
                    "name" if in_name => {
                        in_name = false;
                        let mut author = String::new();
                        if !current_surname.is_empty() {
                            author.push_str(current_surname.trim());
                        }
                        if !current_given.is_empty() {
                            if !author.is_empty() {
                                author.push(' ');
                            }
                            author.push_str(current_given.trim());
                        }
                        if !author.is_empty() {
                            authors.push(author);
                        }
                    }
                    "person-group" => {
                        in_person_group = false;
                    }
                    "element-citation" => {
                        in_element_citation = false;
                    }
                    "mixed-citation" => {
                        in_mixed_citation = false;
                    }
                    _ => {}
                }

                current_tag.clear();
                depth -= 1;
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref()).to_string();
                let trimmed = decoded.trim();

                if !trimmed.is_empty() {
                    budget.check_entity(trimmed)?;
                    budget.account_text(trimmed.len())?;
                    if in_mixed_citation {
                        if !mixed_text.is_empty() {
                            mixed_text.push(' ');
                        }
                        mixed_text.push_str(trimmed);
                    } else if in_element_citation {
                        match current_tag.as_str() {
                            "surname" => current_surname.push_str(trimmed),
                            "given-names" => current_given.push_str(trimmed),
                            "article-title" => {
                                if !article_title.is_empty() {
                                    article_title.push(' ');
                                }
                                article_title.push_str(trimmed);
                            }
                            "source" => source.push_str(trimmed),
                            "year" => year.push_str(trimmed),
                            "volume" => volume.push_str(trimmed),
                            "fpage" => fpage.push_str(trimmed),
                            "lpage" => lpage.push_str(trimmed),
                            "pub-id-doi" => doi.push_str(trimmed),
                            "publisher-name" => publisher_name.push_str(trimmed),
                            "publisher-loc" => publisher_loc.push_str(trimmed),
                            _ => {}
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    if !mixed_text.is_empty() {
        return Ok(mixed_text);
    }

    let mut citation = String::new();

    if !authors.is_empty() {
        citation.push_str(&authors.join(", "));
        citation.push_str(". ");
    }

    if !article_title.is_empty() {
        citation.push_str(&article_title);
        citation.push_str(". ");
    }

    if !source.is_empty() {
        citation.push_str(&source);
        citation.push('.');
    }

    if !year.is_empty() {
        citation.push(' ');
        citation.push_str(&year);
    }
    if !volume.is_empty() {
        citation.push(';');
        citation.push_str(&volume);
    }
    if !fpage.is_empty() {
        citation.push(':');
        citation.push_str(&fpage);
        if !lpage.is_empty() {
            citation.push('-');
            citation.push_str(&lpage);
        }
    }
    if !citation.is_empty() && !citation.ends_with('.') {
        citation.push('.');
    }

    if !publisher_name.is_empty() || !publisher_loc.is_empty() {
        if !citation.is_empty() {
            citation.push(' ');
        }
        if !publisher_loc.is_empty() {
            citation.push_str(&publisher_loc);
        }
        if !publisher_loc.is_empty() && !publisher_name.is_empty() {
            citation.push_str(": ");
        }
        if !publisher_name.is_empty() {
            citation.push_str(&publisher_name);
        }
        citation.push('.');
    }

    if !doi.is_empty() {
        if !citation.is_empty() {
            citation.push(' ');
        }
        citation.push_str("DOI: ");
        citation.push_str(&doi);
        citation.push('.');
    }

    Ok(citation.trim().to_string())
}

/// Extract structured content from a JATS `<fig>` element.
///
/// Parses `<label>`, `<caption>` (with nested `<title>`/`<p>`), and
/// `<graphic xlink:href="...">` so callers can associate the figure's caption
/// text with its graphic reference instead of dropping or flattening them.
///
/// Returns `(label, caption_text, graphic_href)`.
pub(super) fn extract_fig_content(
    reader: &mut EntityReader<'_>,
    budget: &mut SecurityBudget,
) -> Result<(Option<String>, Option<String>, Option<String>)> {
    let mut depth: u32 = 0;
    let mut in_caption = false;
    let mut current_tag = String::new();

    let mut label = String::new();
    let mut caption = String::new();
    let mut href: Option<String> = None;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                depth += 1;
                let name = e.name();
                let tag = crate::utils::xml_tag_name(name.as_ref()).to_string();

                match tag.as_str() {
                    "caption" => in_caption = true,
                    "label" | "title" | "p" => current_tag = tag.clone(),
                    "graphic" => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "xlink:href" || key.ends_with(":href") || key == "href" {
                                href = Some(val.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
            // `<graphic xlink:href="..."/>` is almost always self-closing, which quick-xml
            // reports as a standalone `Empty` event (no matching `Start`/`End` pair), so it
            // must be handled separately from `Event::Start` or the href is silently dropped.
            Ok(Event::Empty(e)) => {
                budget.enter()?;
                budget.leave();
                let name = e.name();
                let tag = crate::utils::xml_tag_name(name.as_ref());
                if tag.as_ref() == "graphic" {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref());
                        let val = String::from_utf8_lossy(attr.value.as_ref());
                        budget.check_attr(&key, &val)?;
                        if key == "xlink:href" || key.ends_with(":href") || key == "href" {
                            href = Some(val.to_string());
                        }
                    }
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                if depth == 0 {
                    break;
                }
                let name = e.name();
                let tag = crate::utils::xml_tag_name(name.as_ref());
                if tag.as_ref() == "caption" {
                    in_caption = false;
                }
                current_tag.clear();
                depth -= 1;
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref()).to_string();
                let trimmed = decoded.trim();

                if !trimmed.is_empty() {
                    budget.check_entity(trimmed)?;
                    budget.account_text(trimmed.len())?;
                    match current_tag.as_str() {
                        "label" => {
                            if !label.is_empty() {
                                label.push(' ');
                            }
                            label.push_str(trimmed);
                        }
                        "title" | "p" if in_caption => {
                            if !caption.is_empty() {
                                caption.push(' ');
                            }
                            caption.push_str(trimmed);
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok((
        if label.is_empty() { None } else { Some(label) },
        if caption.is_empty() { None } else { Some(caption) },
        href,
    ))
}
