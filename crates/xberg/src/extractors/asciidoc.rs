//! AsciiDoc extractor.
//!
//! Parses the structural subset of AsciiDoc that carries document meaning, rather than
//! flattening the source into undifferentiated paragraphs the way the plain-text
//! extractor used to (#228):
//!
//! - **Document header**: `= Title`, the implicit author line, and `:attribute: value`
//!   entries (attributes are also substituted into body text as `{name}` references).
//! - **Sections**: `== `, `=== ` … `====== ` become real headings at the matching level.
//! - **Lists**: `*`/`-` unordered and `.` ordered items, including nesting depth.
//! - **Delimited blocks**: `----` listing blocks (with the language from a preceding
//!   `[source,lang]` attribute line), `....` literal blocks, `____` quote blocks and
//!   `====` example blocks.
//! - **Admonitions**: both the inline `NOTE: text` form and the `[NOTE]` + `====` block
//!   form.
//! - **Tables**: `|===` delimited tables, including multi-line rows, become structured
//!   tables rather than pipe-noise paragraphs.
//! - **Inline markup**: constrained `*strong*`, `_emphasis_`, `` `mono` `` and the
//!   `link:target[text]` / `https://url[text]` macros become text annotations.
//!
//! `TextMetadata.headers`, `.links` and `.code_blocks` are populated from the parse.
//!
//! Malformed input degrades: an unterminated delimited block or table is closed at
//! end-of-input and reported as a [`ProcessingWarning`] instead of failing extraction.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::transform::normalize_line_endings;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ProcessingWarning;
use crate::types::document_structure::{AnnotationKind, TextAnnotation};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;

/// Minimum run length for a block delimiter line (`----`, `....`, `====`, `____`).
const MIN_DELIMITER_RUN: usize = 4;

/// Deepest section level AsciiDoc defines (`====== ` → level 6).
const MAX_SECTION_LEVEL: usize = 6;

/// Inline admonition labels recognised in the `LABEL: text` form.
const ADMONITION_LABELS: [&str; 5] = ["NOTE", "TIP", "IMPORTANT", "WARNING", "CAUTION"];

/// Characters that may appear in an AsciiDoc table cell specifier (`2+|`, `.3+|`, `a|`).
const CELL_SPEC_CHARS: [char; 15] = [
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '+', '*', '.', '<', '>',
];

/// AsciiDoc document extractor.
///
/// Produces headings, lists, code blocks, tables, admonitions and quotes from AsciiDoc
/// source, together with `TextMetadata` carrying the document's headers, links and code
/// blocks.
#[cfg_attr(alef, alef(skip))]
pub struct AsciiDocExtractor;

impl AsciiDocExtractor {
    /// Create a new AsciiDoc extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for AsciiDocExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// Everything a single AsciiDoc parse produces.
struct ParseOutput {
    document: InternalDocument,
    headers: Vec<String>,
    links: Vec<(String, String)>,
    code_blocks: Vec<(String, String)>,
}

/// Line-oriented AsciiDoc parser.
///
/// Held as a struct so each block form gets its own short handler instead of one
/// monolithic loop.
struct AsciiDocParser<'a> {
    lines: Vec<&'a str>,
    index: usize,
    builder: InternalDocumentBuilder,
    /// Open list nesting: one entry per level, `true` when that level is ordered.
    list_stack: Vec<bool>,
    /// Document attributes from `:name: value` entries, used for `{name}` substitution.
    attributes: AHashMap<String, String>,
    /// Comma-separated values of the most recent `[...]` block attribute line.
    pending_attrs: Vec<String>,
    /// Text of the most recent `.Block title` line.
    pending_title: Option<String>,
    headers: Vec<String>,
    links: Vec<(String, String)>,
    code_blocks: Vec<(String, String)>,
    warnings: Vec<ProcessingWarning>,
    title: Option<String>,
    authors: Option<Vec<String>>,
}

impl<'a> AsciiDocParser<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            lines: text.lines().collect(),
            index: 0,
            builder: InternalDocumentBuilder::new("asciidoc"),
            list_stack: Vec::new(),
            attributes: AHashMap::new(),
            pending_attrs: Vec::new(),
            pending_title: None,
            headers: Vec::new(),
            links: Vec::new(),
            code_blocks: Vec::new(),
            warnings: Vec::new(),
            title: None,
            authors: None,
        }
    }

    fn parse(mut self) -> ParseOutput {
        self.parse_document_header();
        while self.index < self.lines.len() {
            self.parse_block();
        }
        self.close_lists();

        let mut metadata = Metadata {
            title: self.title.clone(),
            authors: self.authors.clone(),
            ..Default::default()
        };
        for (key, value) in &self.attributes {
            metadata
                .additional
                .insert(Cow::Owned(format!("asciidoc_{key}")), serde_json::json!(value));
        }
        self.builder.set_metadata(metadata);
        for warning in std::mem::take(&mut self.warnings) {
            self.builder.add_warning(warning);
        }

        ParseOutput {
            document: self.builder.build(),
            headers: self.headers,
            links: self.links,
            code_blocks: self.code_blocks,
        }
    }

    /// Consume the document header: `= Title`, the implicit author line, attribute entries.
    fn parse_document_header(&mut self) {
        self.skip_blank_and_comment_lines();
        let Some(first) = self.lines.get(self.index).copied() else {
            return;
        };
        let Some(rest) = first.strip_prefix("= ") else {
            return;
        };
        let (text, annotations) = self.parse_inline(rest.trim());
        let idx = self.builder.push_title(&text, None, None);
        if !annotations.is_empty() {
            self.builder.set_annotations(idx, annotations);
        }
        self.headers.push(text.clone());
        self.title = Some(text);
        self.index += 1;

        if let Some(author_line) = self.lines.get(self.index).copied() {
            let trimmed = author_line.trim();
            let is_author = !trimmed.is_empty()
                && !trimmed.starts_with(':')
                && !trimmed.starts_with("//")
                && !trimmed.starts_with('=');
            if is_author {
                let authors: Vec<String> = trimmed.split(';').map(|a| a.trim().to_string()).collect();
                self.authors = Some(authors);
                self.index += 1;
            }
        }
        self.consume_attribute_entries();
    }

    /// Consume a run of `:name: value` attribute entries and blank/comment lines.
    fn consume_attribute_entries(&mut self) {
        while let Some(line) = self.lines.get(self.index).copied() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                self.index += 1;
                continue;
            }
            let Some(entry) = parse_attribute_entry(trimmed) else {
                return;
            };
            self.attributes.insert(entry.0, entry.1);
            self.index += 1;
        }
    }

    fn skip_blank_and_comment_lines(&mut self) {
        while let Some(line) = self.lines.get(self.index).copied() {
            let trimmed = line.trim();
            if trimmed.is_empty() || (trimmed.starts_with("//") && !is_delimiter(trimmed, '/')) {
                self.index += 1;
            } else {
                return;
            }
        }
    }

    /// Dispatch a single block starting at the current line.
    fn parse_block(&mut self) {
        let Some(raw) = self.lines.get(self.index).copied() else {
            return;
        };
        let trimmed = raw.trim();

        if trimmed.is_empty() {
            self.index += 1;
            return;
        }
        if is_delimiter(trimmed, '/') {
            self.skip_comment_block();
            return;
        }
        if trimmed.starts_with("//") {
            self.index += 1;
            return;
        }
        if let Some(entry) = parse_attribute_entry(trimmed) {
            self.attributes.insert(entry.0, entry.1);
            self.index += 1;
            return;
        }
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            self.pending_attrs = parse_block_attributes(trimmed);
            self.index += 1;
            return;
        }
        if let Some(title) = parse_block_title(trimmed) {
            let title = self.substitute_attributes(title);
            self.pending_title = Some(title);
            self.index += 1;
            return;
        }
        if trimmed.starts_with("|===") {
            self.parse_table();
            return;
        }
        if let Some(level) = section_level(trimmed) {
            self.parse_section_heading(level, trimmed);
            return;
        }
        if is_delimiter(trimmed, '+')
            && let Some(notation) = self.pending_math_notation()
        {
            self.parse_math_block(trimmed, notation);
            return;
        }
        if is_delimiter(trimmed, '-') || is_delimiter(trimmed, '.') {
            self.parse_verbatim_block(trimmed);
            return;
        }
        if is_delimiter(trimmed, '_') {
            self.parse_quote_block(trimmed);
            return;
        }
        if is_delimiter(trimmed, '=') {
            self.parse_example_block(trimmed);
            return;
        }
        if let Some((label, body)) = split_inline_admonition(trimmed) {
            self.push_admonition_with_body(&label.to_lowercase(), body);
            self.index += 1;
            return;
        }
        if let Some((depth, ordered)) = list_marker(trimmed) {
            self.parse_list_item(depth, ordered, trimmed);
            return;
        }
        self.parse_paragraph();
    }

    fn parse_section_heading(&mut self, level: usize, trimmed: &str) {
        self.close_lists();
        let text = trimmed[level..].trim();
        let (text, annotations) = self.parse_inline(text);
        let idx = self.builder.push_heading(level as u8, &text, None, None);
        if !annotations.is_empty() {
            self.builder.set_annotations(idx, annotations);
        }
        self.headers.push(text);
        self.index += 1;
        self.pending_attrs.clear();
        self.pending_title = None;
    }

    /// The math notation a pending `[latexmath]`, `[asciimath]`, or `[stem]`
    /// block attribute selects.
    ///
    /// `stem` follows the document's `:stem:` attribute, which AsciiDoc defines
    /// as AsciiMath unless the document names `latexmath`.
    fn pending_math_notation(&self) -> Option<MathNotation> {
        self.pending_attrs.first().and_then(|attr| {
            match attr.split(',').next().unwrap_or("").trim().to_ascii_lowercase().as_str() {
                "latexmath" => Some(MathNotation::Latex),
                "asciimath" => Some(MathNotation::AsciiMath),
                "stem" => Some(self.stem_notation()),
                _ => None,
            }
        })
    }

    /// What `stem` means in this document.
    fn stem_notation(&self) -> MathNotation {
        match self.attributes.get("stem").map(|v| v.trim().to_ascii_lowercase()) {
            Some(value) if value == "latexmath" || value == "latex" => MathNotation::Latex,
            _ => MathNotation::AsciiMath,
        }
    }

    /// Parse a `++++` passthrough block that a math attribute introduced.
    ///
    /// The body is one display equation, so it becomes a formula element.
    fn parse_math_block(&mut self, delimiter: &str, notation: MathNotation) {
        self.close_lists();
        self.index += 1;
        let (body, terminated) = self.collect_until_delimiter(delimiter);
        if !terminated {
            self.warn("unterminated delimited block closed at end of input");
        }
        let latex = self.math_to_latex(body.trim(), notation);
        if let Some(latex) = latex
            && !latex.is_empty()
        {
            self.builder.push_formula(&latex, None, None);
        }
        self.pending_attrs.clear();
        self.pending_title = None;
    }

    /// Convert math in `notation` to LaTeX.
    ///
    /// LaTeX passes through with its delimiters removed, since the formula
    /// element holds bare LaTeX. AsciiMath goes through the shared converter.
    fn math_to_latex(&mut self, source: &str, notation: MathNotation) -> Option<String> {
        if source.is_empty() {
            return None;
        }
        match notation {
            MathNotation::Latex => {
                let bare = crate::extraction::derive::strip_math_delimiters(source).trim();
                if bare.is_empty() { None } else { Some(bare.to_string()) }
            }
            // The AsciiMath converter needs `mathemascii`, which `office` brings
            // in. A build without it keeps the block as its own notation rather
            // than dropping the mathematics.
            #[cfg(feature = "office")]
            MathNotation::AsciiMath => {
                let mut budget = SecurityBudget::with_defaults();
                crate::extraction::asciimath::convert_asciimath_to_latex(source, &mut budget)
            }
            #[cfg(not(feature = "office"))]
            MathNotation::AsciiMath => Some(source.to_string()),
        }
    }

    /// Parse a `----` listing block or a `....` literal block into a code element.
    fn parse_verbatim_block(&mut self, delimiter: &str) {
        self.close_lists();
        let language = self.pending_source_language();
        let title = self.pending_title.take();
        self.index += 1;
        let (body, terminated) = self.collect_until_delimiter(delimiter);
        if !terminated {
            self.warn("unterminated delimited block closed at end of input");
        }
        let idx = self.builder.push_code(&body, language.as_deref(), None, None);
        if let Some(title) = title {
            self.builder.set_attributes(idx, single_attribute("title", &title));
        }
        self.code_blocks.push((language.unwrap_or_default(), body));
        self.pending_attrs.clear();
    }

    fn parse_quote_block(&mut self, delimiter: &str) {
        self.close_lists();
        self.index += 1;
        let (body, terminated) = self.collect_until_delimiter(delimiter);
        if !terminated {
            self.warn("unterminated quote block closed at end of input");
        }
        self.builder.push_quote_start();
        for paragraph in body.split("\n\n") {
            let trimmed = paragraph.trim();
            if trimmed.is_empty() {
                continue;
            }
            let (text, annotations) = self.parse_inline(&trimmed.replace('\n', " "));
            self.builder.push_paragraph(&text, annotations, None, None);
        }
        self.builder.push_quote_end();
        self.pending_attrs.clear();
        self.pending_title = None;
    }

    /// Parse an `====` example block. With a preceding `[NOTE]`-style attribute it is an
    /// admonition; otherwise its body is emitted as ordinary paragraphs.
    fn parse_example_block(&mut self, delimiter: &str) {
        self.close_lists();
        let admonition = self.pending_admonition_kind();
        self.index += 1;
        let (body, terminated) = self.collect_until_delimiter(delimiter);
        if !terminated {
            self.warn("unterminated example block closed at end of input");
        }
        let joined = body.split_whitespace().collect::<Vec<_>>().join(" ");
        match admonition {
            Some(kind) => self.push_admonition_with_body(&kind, &joined),
            None => {
                if !joined.is_empty() {
                    let (text, annotations) = self.parse_inline(&joined);
                    self.builder.push_paragraph(&text, annotations, None, None);
                }
            }
        }
        self.pending_attrs.clear();
        self.pending_title = None;
    }

    fn push_admonition_with_body(&mut self, kind: &str, body: &str) {
        self.close_lists();
        let idx = self.builder.push_admonition(kind, None, None);
        let (text, annotations) = self.parse_inline(body.trim());
        if !text.is_empty() {
            self.builder.set_text(idx, &text);
        }
        if !annotations.is_empty() {
            self.builder.set_annotations(idx, annotations);
        }
        self.pending_attrs.clear();
        self.pending_title = None;
    }

    /// Parse a `|===` delimited table into a structured table element.
    fn parse_table(&mut self) {
        self.close_lists();
        let expected_columns = self.pending_column_count();
        let title = self.pending_title.take();
        self.index += 1;

        let mut cells: Vec<String> = Vec::new();
        let mut first_row_width = 0usize;
        let mut terminated = false;
        while let Some(raw) = self.lines.get(self.index).copied() {
            self.index += 1;
            let trimmed = raw.trim();
            if trimmed.starts_with("|===") {
                terminated = true;
                break;
            }
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('|') {
                let row = split_table_row(trimmed);
                if first_row_width == 0 {
                    first_row_width = row.len();
                }
                cells.extend(row.into_iter().map(|c| self.render_cell(&c)));
            } else if let Some(last) = cells.last_mut() {
                last.push(' ');
                last.push_str(trimmed);
            }
        }
        if !terminated {
            self.warn("unterminated table block closed at end of input");
        }

        let columns = expected_columns.unwrap_or(first_row_width).max(1);
        let grid: Vec<Vec<String>> = cells.chunks(columns).map(<[String]>::to_vec).collect();
        if grid.is_empty() {
            self.pending_attrs.clear();
            return;
        }
        let idx = self.builder.push_table_from_cells(&grid, None, None);
        if let Some(title) = title {
            self.builder.set_attributes(idx, single_attribute("title", &title));
        }
        self.pending_attrs.clear();
    }

    fn parse_list_item(&mut self, depth: usize, ordered: bool, trimmed: &str) {
        self.open_lists(depth, ordered);
        let content = trimmed
            .split_once(char::is_whitespace)
            .map_or("", |(_, rest)| rest)
            .trim();
        let mut text = content.to_string();
        self.index += 1;
        while let Some(next) = self.lines.get(self.index).copied() {
            let next_trimmed = next.trim();
            if next_trimmed.is_empty() || list_marker(next_trimmed).is_some() || section_level(next_trimmed).is_some() {
                break;
            }
            text.push(' ');
            text.push_str(next_trimmed);
            self.index += 1;
        }
        let (text, annotations) = self.parse_inline(&text);
        self.builder.push_list_item(&text, ordered, annotations, None, None);
        self.pending_attrs.clear();
        self.pending_title = None;
    }

    fn parse_paragraph(&mut self) {
        self.close_lists();
        let mut text = String::new();
        while let Some(raw) = self.lines.get(self.index).copied() {
            let trimmed = raw.trim();
            if trimmed.is_empty() || self.starts_new_block(trimmed) {
                break;
            }
            if !text.is_empty() {
                text.push(' ');
            }
            text.push_str(trimmed);
            self.index += 1;
        }
        if text.is_empty() {
            self.index += 1;
            return;
        }
        let (text, annotations) = self.parse_inline(&text);
        self.builder.push_paragraph(&text, annotations, None, None);
        self.pending_attrs.clear();
        self.pending_title = None;
    }

    /// True when `trimmed` opens a block that must not be absorbed into a paragraph.
    fn starts_new_block(&self, trimmed: &str) -> bool {
        section_level(trimmed).is_some()
            || list_marker(trimmed).is_some()
            || trimmed.starts_with("|===")
            || trimmed.starts_with("//")
            || parse_attribute_entry(trimmed).is_some()
            || parse_block_title(trimmed).is_some()
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
            || is_delimiter(trimmed, '-')
            || is_delimiter(trimmed, '.')
            || is_delimiter(trimmed, '=')
            || is_delimiter(trimmed, '_')
            || split_inline_admonition(trimmed).is_some()
    }

    /// Collect raw lines until a line equal to `delimiter`. Returns the body and whether
    /// the closing delimiter was actually found.
    fn collect_until_delimiter(&mut self, delimiter: &str) -> (String, bool) {
        let mut body: Vec<&str> = Vec::new();
        while let Some(raw) = self.lines.get(self.index).copied() {
            self.index += 1;
            if raw.trim() == delimiter {
                return (body.join("\n"), true);
            }
            body.push(raw);
        }
        (body.join("\n"), false)
    }

    fn skip_comment_block(&mut self) {
        self.index += 1;
        while let Some(raw) = self.lines.get(self.index).copied() {
            self.index += 1;
            if is_delimiter(raw.trim(), '/') {
                return;
            }
        }
        self.warn("unterminated comment block closed at end of input");
    }

    fn open_lists(&mut self, depth: usize, ordered: bool) {
        while self.list_stack.len() > depth {
            self.builder.end_list();
            self.list_stack.pop();
        }
        if self.list_stack.len() == depth
            && let Some(last) = self.list_stack.last().copied()
            && last != ordered
        {
            self.builder.end_list();
            self.list_stack.pop();
        }
        while self.list_stack.len() < depth {
            self.builder.push_list(ordered);
            self.list_stack.push(ordered);
        }
    }

    fn close_lists(&mut self) {
        while !self.list_stack.is_empty() {
            self.builder.end_list();
            self.list_stack.pop();
        }
    }

    fn pending_source_language(&self) -> Option<String> {
        let mut iter = self.pending_attrs.iter();
        let first = iter.next()?;
        if !first.eq_ignore_ascii_case("source") {
            return None;
        }
        iter.next().filter(|lang| !lang.is_empty()).cloned()
    }

    fn pending_admonition_kind(&self) -> Option<String> {
        let first = self.pending_attrs.first()?;
        ADMONITION_LABELS
            .into_iter()
            .find(|label| first.eq_ignore_ascii_case(label))
            .map(str::to_lowercase)
    }

    fn pending_column_count(&self) -> Option<usize> {
        self.pending_attrs
            .iter()
            .find_map(|attr| attr.strip_prefix("cols="))
            .map(|spec| count_columns(spec.trim_matches('"')))
            .filter(|count| *count > 0)
    }

    fn render_cell(&mut self, raw: &str) -> String {
        self.parse_inline(raw).0
    }

    fn warn(&mut self, message: &'static str) {
        self.warnings.push(ProcessingWarning {
            source: Cow::Borrowed("asciidoc"),
            message: Cow::Borrowed(message),
        });
    }

    /// Replace `{name}` references with defined document attributes.
    ///
    /// Undefined references are left verbatim: AsciiDoc treats them as literal text by
    /// default, and dropping them would silently lose content.
    fn substitute_attributes(&self, text: &str) -> String {
        if !text.contains('{') || self.attributes.is_empty() {
            return text.to_string();
        }
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(open) = rest.find('{') {
            out.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            match after.find('}') {
                Some(close) => {
                    let name = &after[..close];
                    match self.attributes.get(name) {
                        Some(value) => out.push_str(value),
                        None => {
                            out.push('{');
                            out.push_str(name);
                            out.push('}');
                        }
                    }
                    rest = &after[close + 1..];
                }
                None => {
                    out.push('{');
                    rest = after;
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Strip AsciiDoc inline markup, producing plain text plus annotations.
    fn parse_inline(&mut self, raw: &str) -> (String, Vec<TextAnnotation>) {
        let substituted = self.substitute_attributes(raw);
        let mut out = String::with_capacity(substituted.len());
        let mut annotations = Vec::new();
        let mut rest = substituted.as_str();
        // An AsciiDoc constrained span may only open at the start of the text or after
        // whitespace / an opening bracket, so `midword*not*bold` stays literal.
        let mut at_boundary = true;

        while !rest.is_empty() {
            // An inline math macro stays in the sentence, as inline math does
            // for markdown, but reaches the text as LaTeX between `$`
            // delimiters rather than as the raw macro.
            if let Some((consumed, source, notation)) = parse_math_macro(rest, self.stem_notation()) {
                if let Some(latex) = self.math_to_latex(&source, notation) {
                    out.push('$');
                    out.push_str(&latex);
                    out.push('$');
                }
                rest = &rest[consumed..];
                at_boundary = false;
                continue;
            }
            if let Some((consumed, display, url)) = parse_link_macro(rest) {
                let start = out.len() as u32;
                out.push_str(&display);
                annotations.push(TextAnnotation {
                    start,
                    end: out.len() as u32,
                    kind: AnnotationKind::Link {
                        url: url.clone(),
                        title: None,
                    },
                });
                self.links.push((display, url));
                rest = &rest[consumed..];
                at_boundary = false;
                continue;
            }
            if at_boundary && let Some((consumed, inner, kind)) = parse_constrained_span(rest) {
                let start = out.len() as u32;
                out.push_str(&inner);
                annotations.push(TextAnnotation {
                    start,
                    end: out.len() as u32,
                    kind,
                });
                rest = &rest[consumed..];
                at_boundary = false;
                continue;
            }
            let Some(ch) = rest.chars().next() else { break };
            out.push(ch);
            at_boundary = ch.is_whitespace() || ch == '(' || ch == '[';
            rest = &rest[ch.len_utf8()..];
        }

        (out, annotations)
    }
}

/// Which notation a math macro or block carries.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MathNotation {
    Latex,
    AsciiMath,
}

/// Parse an inline math macro: `latexmath:[...]`, `asciimath:[...]`, or
/// `stem:[...]`, whose notation the document's `:stem:` attribute selects.
///
/// Returns the consumed length, the macro's content, and its notation. The
/// content may hold nested brackets, so the scan tracks depth.
fn parse_math_macro(text: &str, stem: MathNotation) -> Option<(usize, String, MathNotation)> {
    let (name, notation) = ["latexmath", "asciimath", "stem"]
        .iter()
        .find(|name| text.starts_with(&format!("{name}:[")))
        .map(|name| {
            let notation = match *name {
                "latexmath" => MathNotation::Latex,
                "asciimath" => MathNotation::AsciiMath,
                _ => stem,
            };
            (*name, notation)
        })?;

    let open = name.len() + 2;
    let mut depth = 1usize;
    for (offset, ch) in text[open..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open + offset + 1, text[open..open + offset].to_string(), notation));
                }
            }
            _ => {}
        }
    }
    None
}

/// Build a one-entry attribute map.
fn single_attribute(key: &str, value: &str) -> AHashMap<String, String> {
    let mut map = AHashMap::new();
    map.insert(key.to_string(), value.to_string());
    map
}

/// `true` when `line` is a run of at least [`MIN_DELIMITER_RUN`] copies of `ch`.
fn is_delimiter(line: &str, ch: char) -> bool {
    line.len() >= MIN_DELIMITER_RUN && line.chars().all(|c| c == ch)
}

/// Section level for a `== Heading` line: the number of leading `=` characters.
fn section_level(line: &str) -> Option<usize> {
    let equals = line.chars().take_while(|c| *c == '=').count();
    if equals == 0 || equals > MAX_SECTION_LEVEL {
        return None;
    }
    let rest = &line[equals..];
    if rest.starts_with(' ') && !rest.trim().is_empty() {
        Some(equals)
    } else {
        None
    }
}

/// Parse a `:name: value` document attribute entry.
fn parse_attribute_entry(line: &str) -> Option<(String, String)> {
    let rest = line.strip_prefix(':')?;
    let (name, value) = rest.split_once(':')?;
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    Some((name.trim_matches('!').to_string(), value.trim().to_string()))
}

/// Split a `[source,rust]` / `[cols="2*",options="header"]` block attribute line.
fn parse_block_attributes(line: &str) -> Vec<String> {
    let inner = line.trim_start_matches('[').trim_end_matches(']');
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in inner.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    parts.push(current.trim().to_string());
    parts
}

/// Parse a `.Block title` line. `. item` (ordered list) and `..` are not titles.
fn parse_block_title(line: &str) -> Option<&str> {
    let rest = line.strip_prefix('.')?;
    let first = rest.chars().next()?;
    if first.is_whitespace() || first == '.' {
        return None;
    }
    Some(rest)
}

/// Split `NOTE: body` into its label and body.
fn split_inline_admonition(line: &str) -> Option<(&'static str, &str)> {
    ADMONITION_LABELS.iter().find_map(|label| {
        line.strip_prefix(label)
            .and_then(|rest| rest.strip_prefix(": "))
            .map(|body| (*label, body.trim()))
    })
}

/// List marker depth (1-based) and orderedness for `* `, `** `, `- `, `. `, `.. `.
fn list_marker(line: &str) -> Option<(usize, bool)> {
    let first = line.chars().next()?;
    let ordered = match first {
        '*' => false,
        '.' => true,
        '-' => {
            return line.strip_prefix("- ").map(|_| (1, false));
        }
        _ => return None,
    };
    let depth = line.chars().take_while(|c| *c == first).count();
    let rest = &line[depth..];
    if depth > 0 && rest.starts_with(' ') && !rest.trim().is_empty() {
        Some((depth, ordered))
    } else {
        None
    }
}

/// Count declared table columns from a `cols` spec such as `">,<,^,"` or `"3*"`.
fn count_columns(spec: &str) -> usize {
    spec.split(',')
        .map(|item| {
            let item = item.trim();
            match item.split_once('*') {
                Some((repeat, _)) => repeat.trim().parse::<usize>().unwrap_or(1),
                None => 1,
            }
        })
        .sum()
}

/// Split a `|a |b |c` table row into cells, discarding cell specifiers such as `2+`.
fn split_table_row(line: &str) -> Vec<String> {
    let mut cells = Vec::new();
    for fragment in line.split('|').skip(1) {
        let trimmed = fragment.trim();
        let is_spec = !trimmed.is_empty()
            && trimmed.len() <= 4
            && trimmed.chars().all(|c| CELL_SPEC_CHARS.contains(&c) || c == 'a');
        if is_spec && trimmed.chars().any(|c| c == '+' || c == '*') {
            continue;
        }
        cells.push(trimmed.to_string());
    }
    cells
}

/// Parse `link:target[text]`, `https://url[text]` or a bare `https://url` at `rest`'s start.
///
/// Returns the consumed byte length, the display text and the target URL.
fn parse_link_macro(rest: &str) -> Option<(usize, String, String)> {
    const SCHEMES: [&str; 4] = ["link:", "https://", "http://", "mailto:"];
    let scheme = SCHEMES.iter().find(|s| rest.starts_with(**s))?;
    let after_scheme = &rest[scheme.len()..];
    let target_end = after_scheme
        .find(|c: char| c.is_whitespace() || c == '[')
        .unwrap_or(after_scheme.len());
    let target = &after_scheme[..target_end];
    if target.is_empty() {
        return None;
    }
    let url = if *scheme == "link:" {
        target.to_string()
    } else {
        format!("{scheme}{target}")
    };

    let remainder = &after_scheme[target_end..];
    if let Some(inner) = remainder.strip_prefix('[')
        && let Some(close) = inner.find(']')
    {
        let display = inner[..close].trim();
        let display = if display.is_empty() {
            url.clone()
        } else {
            display.to_string()
        };
        return Some((scheme.len() + target_end + close + 2, display, url));
    }
    if *scheme == "link:" {
        // `link:` without a `[...]` target block is not a link macro.
        return None;
    }
    // Bare URL with no macro brackets: display the URL itself. Trailing sentence
    // punctuation is not part of the link.
    let url = url.trim_end_matches(['.', ',', ';', ':', ')']).to_string();
    if url.len() <= scheme.len() {
        return None;
    }
    Some((url.len(), url.clone(), url))
}

/// Parse a constrained `*strong*`, `_emphasis_` or `` `mono` `` span at `rest`'s start.
fn parse_constrained_span(rest: &str) -> Option<(usize, String, AnnotationKind)> {
    let marker = rest.chars().next()?;
    let kind = match marker {
        '*' => AnnotationKind::Bold,
        '_' => AnnotationKind::Italic,
        '`' => AnnotationKind::Code,
        _ => return None,
    };
    let after = &rest[marker.len_utf8()..];
    if after.starts_with(char::is_whitespace) {
        return None;
    }
    let close = after.find(marker)?;
    let inner = &after[..close];
    if inner.is_empty() || inner.ends_with(char::is_whitespace) {
        return None;
    }
    Some((marker.len_utf8() * 2 + close, inner.to_string(), kind))
}

impl Plugin for AsciiDocExtractor {
    fn name(&self) -> &str {
        "asciidoc-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Extracts structured content from AsciiDoc documents"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for AsciiDocExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let mut budget = SecurityBudget::from_config(config);
        budget.account_text(content.len())?;

        // See the identical rationale on `extractors::text::PlainTextExtractor::extract_content`
        // (#171, #395): `replaced_characters` is recorded by the decoder itself, at the point
        // data is actually lost, so it stays accurate under `quality`'s mojibake cleanup where
        // scanning the decoded text for U+FFFD afterwards would not. ~keep
        let outcome = crate::utils::decode_with_provenance(content, None);
        let decoded_lossily = outcome.replaced_characters;
        let source = normalize_line_endings(crate::utils::strip_bom(&outcome.text));

        let parsed = AsciiDocParser::new(&source).parse();
        let mut document = parsed.document;
        document.mime_type = mime_type.to_string();
        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut document.processing_warnings,
                "asciidoc",
                "AsciiDoc source",
            );
        }

        let body: String = document
            .elements
            .iter()
            .map(|element| element.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        document.metadata.format = Some(crate::types::FormatMetadata::Text(crate::types::TextMetadata {
            line_count: source.lines().count() as u32,
            word_count: body.split_whitespace().count() as u32,
            character_count: body.chars().count() as u32,
            headers: Some(parsed.headers).filter(|h| !h.is_empty()),
            links: Some(parsed.links).filter(|l| !l.is_empty()),
            code_blocks: Some(parsed.code_blocks).filter(|c| !c.is_empty()),
        }));

        Ok(document)
    }

    fn supported_mime_types(&self) -> &[&str] {
        // `text/x-asciidoc` is declared as an alias of `text/asciidoc` in core/mime.rs.
        // The registry resolves extractors by exact string with no alias awareness, so an
        // unclaimed alias would be advertised as supported and then fail as
        // UnsupportedFormat (#229).
        &["text/asciidoc", "text/x-asciidoc"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::internal::ElementKind;

    fn parse(source: &str) -> InternalDocument {
        let normalized = normalize_line_endings(source);
        AsciiDocParser::new(&normalized).parse().document
    }

    /// The LaTeX of every formula element, in document order.
    fn formulas(source: &str) -> Vec<String> {
        parse(source)
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Formula))
            .map(|e| e.text.clone())
            .collect()
    }

    /// The text of every paragraph, in document order.
    fn paragraphs(source: &str) -> Vec<String> {
        parse(source)
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Paragraph))
            .map(|e| e.text.clone())
            .collect()
    }

    #[test]
    fn should_emit_a_latexmath_block_as_a_formula() {
        let source = "[latexmath]\n++++\n\\int_0^1 x\\,dx = \\frac{1}{2}\n++++\n";
        assert_eq!(formulas(source), vec!["\\int_0^1 x\\,dx = \\frac{1}{2}"]);
    }

    /// A `stem` block is AsciiMath unless the document says otherwise, so it
    /// converts rather than passing through.
    #[cfg(feature = "office")]
    #[test]
    fn should_convert_a_stem_block_from_asciimath() {
        let source = "[stem]\n++++\nsqrt(4) = 2\n++++\n";
        assert_eq!(formulas(source), vec!["\\sqrt{4}=2"]);
    }

    /// An `[asciimath]` block names its notation, whatever `:stem:` says.
    #[cfg(feature = "office")]
    #[test]
    fn should_convert_an_asciimath_block_whatever_the_stem_attribute_says() {
        let source = "= Doc\n:stem: latexmath\n\n[asciimath]\n++++\nsqrt(4) = 2\n++++\n";
        assert_eq!(formulas(source), vec!["\\sqrt{4}=2"]);
    }

    /// Without the feature that provides the converter, an AsciiMath block keeps
    /// its own notation rather than losing the mathematics.
    #[cfg(not(feature = "office"))]
    #[test]
    fn should_keep_asciimath_source_when_the_converter_is_absent() {
        let source = "[asciimath]\n++++\nsqrt(4) = 2\n++++\n";
        assert_eq!(formulas(source), vec!["sqrt(4) = 2"]);
    }

    /// `:stem: latexmath` makes `stem` mean LaTeX, which passes through.
    #[test]
    fn should_treat_a_stem_block_as_latex_when_the_document_says_so() {
        let source = "= Doc\n:stem: latexmath\n\n[stem]\n++++\n\\alpha + \\beta\n++++\n";
        assert_eq!(formulas(source), vec!["\\alpha + \\beta"]);
    }

    /// A passthrough block with no math attribute is not math.
    #[test]
    fn should_leave_a_plain_passthrough_block_alone() {
        let source = "++++\n<hr/>\n++++\n";
        assert!(formulas(source).is_empty());
    }

    /// Inline math stays in the sentence, as it does for markdown, but reaches
    /// the text as delimited LaTeX rather than as the raw macro.
    #[test]
    fn should_render_inline_math_macros_as_delimited_latex() {
        let source = "The value latexmath:[E = mc^2] holds.\n";
        assert_eq!(paragraphs(source), vec!["The value $E = mc^2$ holds."]);
        assert!(formulas(source).is_empty());
    }

    #[cfg(feature = "office")]
    #[test]
    fn should_convert_an_inline_stem_macro_from_asciimath() {
        let source = "Take stem:[sqrt(4)] as given.\n";
        assert_eq!(paragraphs(source), vec!["Take $\\sqrt{4}$ as given."]);
    }

    /// Macro content may hold brackets of its own.
    #[test]
    fn should_read_a_math_macro_containing_brackets() {
        let parsed = parse_math_macro("latexmath:[a[i] + b]rest", MathNotation::AsciiMath);
        let (consumed, source, notation) = parsed.expect("macro parses");
        assert_eq!(source, "a[i] + b");
        assert_eq!(notation, MathNotation::Latex);
        assert_eq!(&"latexmath:[a[i] + b]rest"[consumed..], "rest");
    }

    #[test]
    fn should_map_equals_runs_to_section_levels() {
        assert_eq!(section_level("= Title"), Some(1));
        assert_eq!(section_level("=== Deep"), Some(3));
        assert_eq!(section_level("======= Too deep"), None);
        assert_eq!(section_level("=="), None);
        assert_eq!(section_level("no heading"), None);
    }

    #[test]
    fn should_detect_list_markers_with_depth() {
        assert_eq!(list_marker("* one"), Some((1, false)));
        assert_eq!(list_marker("*** three"), Some((3, false)));
        assert_eq!(list_marker("- dash"), Some((1, false)));
        assert_eq!(list_marker(". first"), Some((1, true)));
        assert_eq!(list_marker(".. second"), Some((2, true)));
        assert_eq!(list_marker(".Block title"), None);
    }

    #[test]
    fn should_count_columns_from_cols_spec() {
        assert_eq!(count_columns(">,<,^,"), 4);
        assert_eq!(count_columns("3*"), 3);
        assert_eq!(count_columns("^20%,<17%,>20%,<43%"), 4);
    }

    #[test]
    fn should_push_document_title_and_headings() {
        let doc = parse("= Doc Title\n\n== Section One\n\nBody text.\n");
        assert_eq!(doc.elements[0].kind, ElementKind::Title);
        assert_eq!(doc.elements[0].text, "Doc Title");
        assert_eq!(doc.elements[1].kind, ElementKind::Heading { level: 2 });
        assert_eq!(doc.elements[1].text, "Section One");
        assert_eq!(doc.elements[2].kind, ElementKind::Paragraph);
        assert_eq!(doc.elements[2].text, "Body text.");
    }

    #[test]
    fn should_extract_source_block_language() {
        let doc = parse("[source,rust]\n----\nfn main() {}\n----\n");
        let code = doc.elements.iter().find(|e| e.kind == ElementKind::Code).expect("code");
        assert_eq!(code.text, "fn main() {}");
        assert_eq!(
            code.attributes
                .as_ref()
                .and_then(|a| a.get("language"))
                .map(String::as_str),
            Some("rust")
        );
    }

    #[test]
    fn should_not_panic_on_unterminated_block() {
        let doc = parse("[source,rust]\n----\nfn main() {}\n");
        assert_eq!(doc.processing_warnings.len(), 1);
        assert!(doc.elements.iter().any(|e| e.kind == ElementKind::Code));
    }

    /// Warnings emitted for a lossy decode specifically, identified by message content
    /// since `"asciidoc"` is also the source of unrelated structural warnings (e.g. an
    /// unterminated block).
    fn decode_warnings(doc: &InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.message.contains("not valid UTF-8"))
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #395: invalid UTF-8 bytes must be reported via `processing_warnings`, not just
    /// silently replaced -- `decode_with_provenance` captures the loss at decode time
    /// so it survives regardless of the `quality` feature's downstream mojibake cleanup.
    ///
    /// Deliberately not run under `quality`: there chardetng resolves these bytes to a
    /// single-byte encoding that maps all of 0x00-0xFF, so nothing is *replaced* -- see
    /// the identical note on `extractors::text::should_warn_when_text_source_is_not_valid_utf8`.
    #[cfg(not(feature = "quality"))]
    #[tokio::test]
    async fn should_warn_when_asciidoc_source_is_not_valid_utf8() {
        let extractor = AsciiDocExtractor::new();
        let config = ExtractionConfig::default();
        let content: &[u8] = &[b'A', 0xFF, 0xFE, b'B'];

        let result = extractor
            .extract_content(content, "text/asciidoc", &config)
            .await
            .expect("extraction of invalid UTF-8 must still succeed");

        let warnings = decode_warnings(&result);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one decode warning, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("replacement character"),
            "warning must describe the lossy decode, got {warnings:?}"
        );
    }

    /// A valid UTF-8 document must not produce a lossy-decode warning.
    #[tokio::test]
    async fn valid_utf8_asciidoc_source_produces_zero_decode_warnings() {
        let extractor = AsciiDocExtractor::new();
        let config = ExtractionConfig::default();
        let content = "= Title\n\nBody text.\n".as_bytes();

        let result = extractor
            .extract_content(content, "text/asciidoc", &config)
            .await
            .expect("extraction should succeed");

        assert!(
            decode_warnings(&result).is_empty(),
            "valid UTF-8 must not warn about a lossy decode, got {:?}",
            decode_warnings(&result)
        );
    }
}
