#[cfg(any(feature = "xml", feature = "office", feature = "hwpx"))]
use std::borrow::Cow;

/// Converts XML tag name bytes to a string, avoiding allocation when possible.
#[cfg(any(feature = "xml", feature = "office", feature = "hwpx"))]
#[inline]
pub(crate) fn xml_tag_name(name: &[u8]) -> Cow<'_, str> {
    String::from_utf8_lossy(name)
}

/// Streaming XML reader that restores pre-quick-xml-0.38 text semantics.
///
/// quick-xml 0.38 split text nodes at entity and character references, emitting
/// `Event::GeneralRef` between `Event::Text` fragments and delegating reference
/// resolution to the consumer (tafia/quick-xml#766). Consumers that only match
/// `Event::Text` silently drop the referenced characters (`&amp;` → nothing), and
/// trim-and-join accumulators corrupt spacing around the fragments. This wrapper
/// does for streaming reads what quick-xml's own serde deserializer does
/// internally: it coalesces each run of `Text`/`GeneralRef` events into a single
/// `Event::Text` with references resolved, so consumers see whole text nodes again.
///
/// Reader-level `trim_text` must stay off (the default) — trimming individual
/// fragments before coalescing would destroy the whitespace around references.
#[cfg(any(feature = "xml", feature = "office", feature = "hwpx"))]
pub(crate) struct EntityReader<'x> {
    reader: quick_xml::Reader<&'x [u8]>,
    pending: Option<quick_xml::events::Event<'x>>,
}

#[cfg(any(feature = "xml", feature = "office", feature = "hwpx"))]
// `from_str`, `config_mut`, and `buffer_position` are used only by some of the
// xml/office/docx extractor paths; in feature combos that enable one of `xml`/`office`
// but not the callers (e.g. the default `xberg-ffi` surface), they are unused. They are
// live in other builds, so allow the conditional dead code rather than misgate per method.
#[allow(dead_code)]
impl<'x> EntityReader<'x> {
    pub(crate) fn from_str(content: &'x str) -> Self {
        Self {
            reader: quick_xml::Reader::from_str(content),
            pending: None,
        }
    }

    pub(crate) fn from_bytes(content: &'x [u8]) -> Self {
        Self {
            reader: quick_xml::Reader::from_reader(content),
            pending: None,
        }
    }

    pub(crate) fn config_mut(&mut self) -> &mut quick_xml::reader::Config {
        self.reader.config_mut()
    }

    pub(crate) fn buffer_position(&self) -> u64 {
        self.reader.buffer_position()
    }

    /// Read the next event, merging consecutive `Text` / `GeneralRef` events into
    /// one owned `Event::Text` whose content has all references resolved.
    ///
    /// The returned `Event::Text` carries already-resolved content: read it raw
    /// (`as_ref()` / `decode()`). Never call `unescape()` on it — the content is
    /// no longer escaped, so a resolved `&` would be misparsed as a dangling
    /// entity reference.
    pub(crate) fn read_event(&mut self) -> quick_xml::Result<quick_xml::events::Event<'x>> {
        use quick_xml::events::{BytesText, Event};

        let first = match self.pending.take() {
            Some(event) => event,
            None => self.reader.read_event()?,
        };
        let mut text = match first {
            Event::Text(t) => String::from_utf8_lossy(t.as_ref()).into_owned(),
            Event::GeneralRef(r) => resolve_general_ref(&r),
            other => return Ok(other),
        };
        loop {
            match self.reader.read_event()? {
                Event::Text(t) => text.push_str(&String::from_utf8_lossy(t.as_ref())),
                Event::GeneralRef(r) => text.push_str(&resolve_general_ref(&r)),
                other => {
                    self.pending = Some(other);
                    break;
                }
            }
        }
        Ok(Event::Text(BytesText::from_escaped(text)))
    }
}

/// Resolve an XML general reference (entity or character reference) to its text.
///
/// Resolution is upstream's: [`BytesRef::resolve_char_ref`] for `&#...;` character
/// references and [`quick_xml::escape::resolve_predefined_entity`] for the five
/// XML predefined entities — the same building blocks quick-xml's own serde
/// deserializer resolves references with. On top of that, `&nbsp;` (an HTML
/// entity that real-world FB2/DocBook files use without declaring) resolves to
/// U+00A0, and undeclared DTD entities resolve to an empty string: extraction
/// is best-effort, so an unresolvable reference must not fail the document.
#[cfg(any(feature = "xml", feature = "office", feature = "hwpx"))]
pub(crate) fn resolve_general_ref(reference: &quick_xml::events::BytesRef<'_>) -> String {
    if let Ok(Some(ch)) = reference.resolve_char_ref() {
        return ch.to_string();
    }
    let Ok(name) = reference.decode() else {
        return String::new();
    };
    if let Some(resolved) = quick_xml::escape::resolve_predefined_entity(&name) {
        return resolved.to_string();
    }
    if name.as_ref() == "nbsp" {
        return "\u{00A0}".to_string();
    }
    String::new()
}

#[cfg(all(test, any(feature = "xml", feature = "office")))]
mod tests {
    use super::*;
    use quick_xml::events::{BytesRef, Event};

    fn resolve(name: &str) -> String {
        resolve_general_ref(&BytesRef::new(name))
    }

    #[test]
    fn test_resolve_general_ref_predefined_entities() {
        assert_eq!(resolve("amp"), "&");
        assert_eq!(resolve("lt"), "<");
        assert_eq!(resolve("gt"), ">");
        assert_eq!(resolve("quot"), "\"");
        assert_eq!(resolve("apos"), "'");
        assert_eq!(resolve("nbsp"), "\u{00A0}");
    }

    #[test]
    fn test_resolve_general_ref_character_references() {
        assert_eq!(resolve("#8212"), "\u{2014}");
        assert_eq!(resolve("#x2014"), "\u{2014}");
        assert_eq!(resolve("#65"), "A");
    }

    #[test]
    fn test_resolve_general_ref_unknown_entity_is_empty() {
        assert_eq!(resolve("unknownentity"), "");
        assert_eq!(resolve("#xZZ"), "");
        assert_eq!(resolve("#1114112"), ""); // beyond char::MAX ~keep
    }

    /// The core contract: a text node split at references arrives as ONE Text
    /// event with the references resolved in place, spacing intact.
    #[test]
    fn test_entity_reader_coalesces_text_and_references() {
        let xml = "<root><a>Profits &amp; losses</a><b>5&gt;3</b><c/></root>";
        let mut reader = EntityReader::from_str(xml);
        let mut texts = Vec::new();
        loop {
            match reader.read_event().expect("valid XML") {
                Event::Text(t) => texts.push(String::from_utf8_lossy(t.as_ref()).into_owned()),
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(texts, vec!["Profits & losses", "5>3"]);
    }

    #[test]
    fn test_entity_reader_preserves_non_text_events() {
        let xml = "<root attr=\"v\">x&#65;y<child/></root>";
        let mut reader = EntityReader::from_str(xml);
        let mut summary = Vec::new();
        loop {
            match reader.read_event().expect("valid XML") {
                Event::Start(e) => summary.push(format!("start:{}", String::from_utf8_lossy(e.name().as_ref()))),
                Event::Empty(e) => summary.push(format!("empty:{}", String::from_utf8_lossy(e.name().as_ref()))),
                Event::End(e) => summary.push(format!("end:{}", String::from_utf8_lossy(e.name().as_ref()))),
                Event::Text(t) => summary.push(format!("text:{}", String::from_utf8_lossy(t.as_ref()))),
                Event::Eof => break,
                _ => {}
            }
        }
        assert_eq!(summary, vec!["start:root", "text:xAy", "empty:child", "end:root"]);
    }
}

/// Return the offset of the `>` that closes a `<!DOCTYPE` declaration.
///
/// `tail` starts just after `<!DOCTYPE`. An internal subset may hold a `>`
/// inside its brackets, so the scan tracks bracket depth and returns the first
/// `>` outside them. A declaration that never closes returns `None`.
#[cfg(any(feature = "xml", feature = "office", feature = "hwpx"))]
pub(crate) fn doctype_end(tail: &str) -> Option<usize> {
    let mut bracket_depth: usize = 0;
    for (idx, ch) in tail.char_indices() {
        match ch {
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '>' if bracket_depth == 0 => return Some(idx),
            _ => {}
        }
    }
    None
}

/// Return `xml` without its `<!DOCTYPE ...>` declaration.
///
/// A parser that rejects a DTD cannot read a document that carries one, and an
/// embedded formula object always does. Removing the declaration keeps the
/// rejection in place for the entities it exists to guard against.
#[cfg(any(feature = "xml", feature = "office", feature = "hwpx"))]
pub(crate) fn strip_doctype(xml: &str) -> std::borrow::Cow<'_, str> {
    let Some(start) = xml.find("<!DOCTYPE") else {
        return std::borrow::Cow::Borrowed(xml);
    };
    let tail = &xml[start + "<!DOCTYPE".len()..];
    let Some(end) = doctype_end(tail) else {
        return std::borrow::Cow::Borrowed(xml);
    };
    let mut out = String::with_capacity(xml.len());
    out.push_str(&xml[..start]);
    out.push_str(&tail[end + 1..]);
    std::borrow::Cow::Owned(out)
}
