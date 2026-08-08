//! Native Rust LaTeX text extractor.
//!
//! This extractor provides comprehensive LaTeX document parsing and text extraction.
//!
//! Features:
//! - Metadata extraction: title, author, date from \title{}, \author{}, \date{}
//! - Section hierarchy: \section{}, \subsection{}, \subsubsection{}, etc.
//! - Inline formatting: \emph{}, \textbf{}, \textit{}, \texttt{}, \underline{}
//! - Lists: itemize, enumerate, description environments
//! - Tables: tabular environment parsing
//! - Math: inline ($...$) and display (\[...\]) math preservation
//! - Unicode support
//!
//! Requires the `office` feature.

mod commands;
mod environments;
mod metadata;
mod parser;
mod utilities;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::document_structure::{AnnotationKind, TextAnnotation};
use crate::types::internal::InternalDocument;
use crate::types::internal::{RelationshipKind, RelationshipTarget};
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::uri::ExtractedUri;
use crate::types::{Metadata, Table};
use async_trait::async_trait;

use std::sync::LazyLock;

use parser::LatexParser;
use utilities::{collect_environment, extract_env_name, extract_heading_title};

/// Heading command → level map for documents that contain `\chapter` commands.
/// Chapter occupies level 1; section is demoted to level 2, etc.
static HEADING_LEVELS_WITH_CHAPTERS: LazyLock<ahash::AHashMap<&'static str, u8>> = LazyLock::new(|| {
    let mut m = ahash::AHashMap::with_capacity(10);
    m.insert("chapter", 1);
    m.insert("chapter*", 1);
    m.insert("section", 2);
    m.insert("section*", 2);
    m.insert("subsection", 3);
    m.insert("subsection*", 3);
    m.insert("subsubsection", 4);
    m.insert("subsubsection*", 4);
    m.insert("paragraph", 5);
    m.insert("paragraph*", 5);
    m
});

/// Heading command → level map for documents without `\chapter` commands.
/// Section starts at level 1.
static HEADING_LEVELS_NO_CHAPTERS: LazyLock<ahash::AHashMap<&'static str, u8>> = LazyLock::new(|| {
    let mut m = ahash::AHashMap::with_capacity(8);
    m.insert("section", 1);
    m.insert("section*", 1);
    m.insert("subsection", 2);
    m.insert("subsection*", 2);
    m.insert("subsubsection", 3);
    m.insert("subsubsection*", 3);
    m.insert("paragraph", 4);
    m.insert("paragraph*", 4);
    m
});

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const LATEX_WARNING_SOURCE: &str = "latex";

/// LaTeX document extractor
#[cfg_attr(alef, alef(skip))]
pub struct LatexExtractor;

impl LatexExtractor {
    /// Create a new LaTeX extractor.
    pub(crate) fn new() -> Self {
        Self
    }

    /// Parse LaTeX content and extract text.
    pub(crate) fn extract_from_latex(content: &str) -> (String, Metadata, Vec<Table>) {
        let mut parser = LatexParser::new(content);
        parser.parse()
    }

    /// Strip inline LaTeX formatting commands from text, returning the plain text
    /// and a list of `TextAnnotation`s referencing byte offsets in the output.
    ///
    /// Handles: `\textbf`, `\emph`, `\textit`, `\underline`, `\texttt`, `\href`.
    fn strip_inline_commands(input: &str) -> (String, Vec<TextAnnotation>) {
        let mut output = String::with_capacity(input.len());
        let mut annotations = Vec::new();
        let bytes = input.as_bytes();
        let len = bytes.len();
        let mut pos = 0;

        while pos < len {
            if bytes[pos] == b'\\' {
                if let Some((kind, content, new_pos)) = Self::try_parse_inline_command(&input[pos..]) {
                    let start = output.len() as u32;
                    let (inner_text, inner_anns) = Self::strip_inline_commands(&content);
                    output.push_str(&inner_text);
                    let end = output.len() as u32;
                    for mut ann in inner_anns {
                        ann.start += start;
                        ann.end += start;
                        annotations.push(ann);
                    }
                    if start < end {
                        annotations.push(TextAnnotation { start, end, kind });
                    }
                    pos += new_pos;
                    continue;
                }

                if let Some((replacement, consumed)) = Self::try_parse_special_command(&input[pos..]) {
                    output.push_str(&replacement);
                    pos += consumed;
                    continue;
                }

                if let Some((plain, consumed)) = Self::try_skip_unknown_command(&input[pos..]) {
                    if !plain.is_empty() {
                        let (inner_text, inner_anns) = Self::strip_inline_commands(&plain);
                        let start = output.len() as u32;
                        output.push_str(&inner_text);
                        for mut ann in inner_anns {
                            ann.start += start;
                            ann.end += start;
                            annotations.push(ann);
                        }
                    }
                    pos += consumed;
                    continue;
                }

                output.push('\\');
                pos += 1;
            } else if bytes[pos] == b'$' {
                output.push('$');
                pos += 1;
                while pos < len && bytes[pos] != b'$' {
                    let ch = input[pos..].chars().next().unwrap();
                    output.push(ch);
                    pos += ch.len_utf8();
                }
                if pos < len {
                    output.push('$');
                    pos += 1;
                }
            } else if bytes[pos] == b'-' && pos + 2 < len && bytes[pos + 1] == b'-' && bytes[pos + 2] == b'-' {
                output.push('\u{2014}');
                pos += 3;
            } else if bytes[pos] == b'-' && pos + 1 < len && bytes[pos + 1] == b'-' {
                output.push('\u{2013}');
                pos += 2;
            } else if bytes[pos] == b'`' && pos + 1 < len && bytes[pos + 1] == b'`' {
                output.push('\u{201C}');
                pos += 2;
            } else if bytes[pos] == b'\'' && pos + 1 < len && bytes[pos + 1] == b'\'' {
                output.push('\u{201D}');
                pos += 2;
            } else if bytes[pos] == b'`' {
                output.push('\u{2018}');
                pos += 1;
            } else if bytes[pos] == b'\'' {
                output.push('\u{2019}');
                pos += 1;
            } else {
                let ch = input[pos..].chars().next().unwrap();
                output.push(ch);
                pos += ch.len_utf8();
            }
        }

        (output, annotations)
    }

    /// Try to parse an inline formatting command at the start of `text`.
    ///
    /// Returns `Some((kind, braced_content, bytes_consumed))` on success.
    fn try_parse_inline_command(text: &str) -> Option<(AnnotationKind, String, usize)> {
        let commands: &[(&str, AnnotationKind)] = &[
            ("\\textbf{", AnnotationKind::Bold),
            ("\\emph{", AnnotationKind::Italic),
            ("\\textit{", AnnotationKind::Italic),
            ("\\underline{", AnnotationKind::Underline),
            ("\\texttt{", AnnotationKind::Code),
        ];

        for (prefix, kind) in commands {
            if let Some(after) = text.strip_prefix(prefix)
                && let Some((content, consumed)) = Self::read_braced_content(after)
            {
                return Some((kind.clone(), content, prefix.len() + consumed));
            }
        }

        if let Some(after_href) = text.strip_prefix("\\href{")
            && let Some((url, url_consumed)) = Self::read_braced_content(after_href)
        {
            let after_url = &after_href[url_consumed..];
            if let Some(after_brace) = after_url.strip_prefix('{')
                && let Some((link_text, text_consumed)) = Self::read_braced_content(after_brace)
            {
                let total = "\\href{".len() + url_consumed + 1 + text_consumed;
                return Some((AnnotationKind::Link { url, title: None }, link_text, total));
            }
        }

        if let Some(after_url_cmd) = text.strip_prefix("\\url{")
            && let Some((url, consumed)) = Self::read_braced_content(after_url_cmd)
        {
            let total = "\\url{".len() + consumed;
            return Some((
                AnnotationKind::Link {
                    url: url.clone(),
                    title: None,
                },
                url,
                total,
            ));
        }

        if let Some(after_verb) = text.strip_prefix("\\verb")
            && let Some(delim) = after_verb.chars().next()
            && !delim.is_alphabetic()
            && delim != '{'
        {
            let after_delim = &after_verb[delim.len_utf8()..];
            if let Some(end_pos) = after_delim.find(delim) {
                let content = after_delim[..end_pos].to_string();
                let total = "\\verb".len() + delim.len_utf8() + end_pos + delim.len_utf8();
                return Some((AnnotationKind::Code, content, total));
            }
        }

        None
    }

    /// Try to parse a special character command at the start of `text`.
    ///
    /// Returns `Some((replacement_string, bytes_consumed))` on success.
    fn try_parse_special_command(text: &str) -> Option<(String, usize)> {
        let braced_replacements: &[(&str, &str)] = &[
            ("\\textgreater{}", ">"),
            ("\\textless{}", "<"),
            ("\\textbackslash{}", "\\"),
            ("\\ldots{}", "\u{2026}"),
            ("\\textendash{}", "\u{2013}"),
            ("\\textemdash{}", "\u{2014}"),
            ("\\textasciitilde{}", "~"),
            ("\\textasciicircum{}", "^"),
            ("\\textbar{}", "|"),
        ];

        for (prefix, replacement) in braced_replacements {
            if text.starts_with(prefix) {
                return Some((replacement.to_string(), prefix.len()));
            }
        }

        let simple_replacements: &[(&str, &str)] = &[
            ("\\ldots", "\u{2026}"),
            ("\\dots", "\u{2026}"),
            ("\\&", "&"),
            ("\\#", "#"),
            ("\\_", "_"),
            ("\\{", "{"),
            ("\\}", "}"),
            ("\\%", "%"),
            ("\\$", "$"),
            ("\\\\", "\n"),
            ("\\,", "\u{2009}"),
            ("\\;", " "),
            ("\\!", ""),
            ("\\~", "~"),
            ("\\^{}", "^"),
        ];

        for (prefix, replacement) in simple_replacements {
            if text.starts_with(prefix) {
                return Some((replacement.to_string(), prefix.len()));
            }
        }

        if let Some(after) = text.strip_prefix("\\ensuremath{")
            && let Some((content, consumed)) = Self::read_braced_content(after)
        {
            return Some((content, "\\ensuremath{".len() + consumed));
        }

        None
    }

    /// Try to skip an unknown command at the start of `text`.
    ///
    /// If the command has a braced argument, return its content as plain text.
    /// Otherwise, skip just the command name.
    ///
    /// Returns `Some((extracted_text, bytes_consumed))`.
    fn try_skip_unknown_command(text: &str) -> Option<(String, usize)> {
        if !text.starts_with('\\') {
            return None;
        }

        let after_backslash = &text[1..];
        let cmd_end = after_backslash
            .find(|c: char| !c.is_alphabetic())
            .unwrap_or(after_backslash.len());

        if cmd_end == 0 {
            return None;
        }

        let total_cmd = 1 + cmd_end;

        let rest = &text[total_cmd..];
        let mut consumed = total_cmd;
        let rest = if rest.starts_with('[') {
            if let Some(bracket_end) = rest.find(']') {
                consumed += bracket_end + 1;
                &text[consumed..]
            } else {
                rest
            }
        } else {
            rest
        };

        if let Some(inner) = rest.strip_prefix('{')
            && let Some((content, brace_consumed)) = Self::read_braced_content(inner)
        {
            consumed += 1 + brace_consumed;
            return Some((content, consumed));
        }

        Some((String::new(), consumed))
    }

    /// Read braced content starting after an opening `{` has already been consumed
    /// by the prefix match. The input starts at the first character inside the braces.
    ///
    /// Returns `(content, bytes_consumed_including_closing_brace)`.
    fn read_braced_content(input: &str) -> Option<(String, usize)> {
        let mut depth: u32 = 1;
        let mut content = String::new();
        let mut pos = 0;
        let bytes = input.as_bytes();

        while pos < bytes.len() {
            let ch = input[pos..].chars().next()?;
            let ch_len = ch.len_utf8();
            match ch {
                '{' => {
                    depth += 1;
                    content.push(ch);
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((content, pos + ch_len));
                    }
                    content.push(ch);
                }
                _ => content.push(ch),
            }
            pos += ch_len;
        }
        None
    }

    /// Extract the path from `\includegraphics[opts]{path}` or `\includegraphics{path}`.
    fn extract_includegraphics_path(line: &str) -> Option<String> {
        let prefix = "\\includegraphics";
        let start = line.find(prefix)?;
        let after = &line[start + prefix.len()..];
        let rest = if after.starts_with('[') {
            let bracket_end = after.find(']')?;
            &after[bracket_end + 1..]
        } else {
            after
        };
        if !rest.starts_with('{') {
            return None;
        }
        let inner = &rest[1..];
        let end = inner.find('}')?;
        let path = inner[..end].trim();
        if path.is_empty() { None } else { Some(path.to_string()) }
    }

    /// Extract the text from `\caption{text}`.
    fn extract_caption(content: &str) -> Option<String> {
        let prefix = "\\caption{";
        let start = content.find(prefix)?;
        let after = &content[start + prefix.len()..];
        Self::read_braced_content(after).map(|(text, _)| text)
    }

    /// Build an `InternalDocument` from LaTeX source.
    ///
    /// Captures `\label{}` as anchors, `\ref{}` as CrossReference relationships,
    /// `\cite{}` as CitationReference relationships, and footnotes.
    pub(crate) fn build_internal_document(source: &str, inject_placeholders: bool) -> InternalDocument {
        let mut b = InternalDocumentBuilder::new("latex");
        let lines: Vec<&str> = source.lines().collect();
        let mut in_document = false;
        let is_plain_tex = source.contains("\\bye") && !source.contains("\\begin{document}");
        if is_plain_tex {
            in_document = true;
        }

        let has_chapters = source.contains("\\chapter{") || source.contains("\\chapter*{");
        let heading_map = if has_chapters {
            &*HEADING_LEVELS_WITH_CHAPTERS
        } else {
            &*HEADING_LEVELS_NO_CHAPTERS
        };

        let mut metadata_entries: Vec<(String, String)> = Vec::new();
        for &cmd in &["title", "author", "date"] {
            if let Some(value) = utilities::extract_braced(source, cmd)
                && !value.is_empty()
            {
                metadata_entries.push((cmd.to_string(), value));
            }
        }
        if !metadata_entries.is_empty() {
            b.push_metadata_block(&metadata_entries, None);
        }

        let mut i = 0;

        while i < lines.len() {
            let trimmed = lines[i].trim();

            if is_plain_tex && trimmed.contains("\\bye") {
                break;
            }
            if !is_plain_tex && trimmed.contains("\\begin{document}") {
                in_document = true;
                i += 1;
                continue;
            }
            if !is_plain_tex && trimmed.contains("\\end{document}") {
                break;
            }
            if !in_document {
                i += 1;
                continue;
            }

            if (trimmed.contains("\\begin{") || trimmed.contains("\\begin {"))
                && let Some(env_name) = extract_env_name(trimmed)
            {
                match env_name.as_str() {
                    "itemize" | "enumerate" | "description" => {
                        let ordered = env_name == "enumerate";
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        b.push_list(ordered);
                        Self::build_internal_list_items(&mut b, &env_content, ordered);
                        b.end_list();
                        i = new_i;
                        continue;
                    }
                    "tabular" | "longtable" | "tabularx" | "tabulary" => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        let cells = Self::parse_tabular_cells(&env_content);
                        if !cells.is_empty() {
                            let caption = Self::extract_caption(&env_content);
                            let label = Self::extract_label(&env_content);
                            let idx = b.push_table_from_cells(&cells, None, None);
                            if let Some(lbl) = label {
                                b.set_anchor(idx, &lbl);
                            }
                            if let Some(cap) = caption {
                                let cap_idx = b.push_paragraph(&cap, vec![], None, None);
                                b.push_relationship(cap_idx, RelationshipTarget::Index(idx), RelationshipKind::Caption);
                            }
                        }
                        i = new_i;
                        continue;
                    }
                    "table" => {
                        let (env_content, new_i) = collect_environment(&lines, i, "table");
                        let caption = Self::extract_caption(&env_content);
                        let label = Self::extract_label(&env_content);
                        let end_tag = "\\end{tabular}";
                        if env_content.contains("\\begin{tabular}")
                            && let Some(start) = env_content.find("\\begin{tabular}")
                            && let Some(end) = env_content.find(end_tag)
                        {
                            let tabular_content = &env_content[start..end + end_tag.len()];
                            let inner_lines: Vec<&str> = tabular_content.lines().collect();
                            let (inner_content, _) = collect_environment(&inner_lines, 0, "tabular");
                            let cells = Self::parse_tabular_cells(&inner_content);
                            if !cells.is_empty() {
                                let idx = b.push_table_from_cells(&cells, None, None);
                                if let Some(lbl) = label {
                                    b.set_anchor(idx, &lbl);
                                }
                                if let Some(cap) = caption {
                                    let cap_idx = b.push_paragraph(&cap, vec![], None, None);
                                    b.push_relationship(
                                        cap_idx,
                                        RelationshipTarget::Index(idx),
                                        RelationshipKind::Caption,
                                    );
                                }
                            }
                        }
                        i = new_i;
                        continue;
                    }
                    "figure" => {
                        let (env_content, new_i) = collect_environment(&lines, i, "figure");
                        let caption = Self::extract_caption(&env_content);
                        let label = Self::extract_label(&env_content);
                        if let Some(path) = Self::extract_includegraphics_path(&env_content) {
                            b.push_uri(ExtractedUri::image(&path, caption.clone()));
                            if inject_placeholders {
                                let idx = b.push_paragraph(&format!("[image: {}]", path), vec![], None, None);
                                if let Some(lbl) = label {
                                    b.set_anchor(idx, &lbl);
                                }
                                if let Some(cap) = caption {
                                    let cap_idx = b.push_paragraph(&cap, vec![], None, None);
                                    b.push_relationship(
                                        cap_idx,
                                        RelationshipTarget::Index(idx),
                                        RelationshipKind::Caption,
                                    );
                                }
                            }
                        } else {
                            // No `\includegraphics`: the whole `figure` body (a caption-only
                            // figure, a sub-figure wrapper, a `\resizebox`'d graphic, ...) was
                            // otherwise dropped without a trace, since this arm's only other
                            // output path is the image placeholder above (#171).
                            b.add_warning(crate::core::diagnostics::warning(
                                LATEX_WARNING_SOURCE,
                                "A figure environment without \\includegraphics was skipped; \
                                 its body and caption are missing from the extracted text",
                            ));
                        }
                        i = new_i;
                        continue;
                    }
                    "equation" | "equation*" | "align" | "align*" | "gather" | "gather*" | "multline" | "multline*"
                    | "eqnarray" | "eqnarray*" | "math" | "displaymath" | "flalign" | "flalign*" | "cases" => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        let formula_text = format!("\\begin{{{}}}\n{}\\end{{{}}}", env_name, env_content, env_name);
                        let idx = b.push_formula(&formula_text, None, None);
                        if let Some(lbl) = Self::extract_label(&env_content) {
                            b.set_anchor(idx, &lbl);
                        }
                        i = new_i;
                        continue;
                    }
                    "lstlisting" | "verbatim" | "minted" | "Verbatim" => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        let language = if env_name == "lstlisting" || env_name == "minted" {
                            Self::extract_code_language(trimmed)
                        } else {
                            None
                        };
                        b.push_code(env_content.trim(), language, None, None);
                        i = new_i;
                        continue;
                    }
                    "quote" | "quotation" => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        b.push_quote_start();
                        let inner_lines: Vec<&str> = env_content.lines().collect();
                        Self::build_internal_body(&mut b, &inner_lines, heading_map, inject_placeholders);
                        b.push_quote_end();
                        i = new_i;
                        continue;
                    }
                    "obeylines" => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        for line in env_content.lines() {
                            let line_trimmed = line.trim();
                            if !line_trimmed.is_empty() {
                                let (text, annotations) = Self::strip_inline_commands(line_trimmed);
                                if !text.is_empty() {
                                    b.push_paragraph(&text, annotations, None, None);
                                }
                            }
                        }
                        i = new_i;
                        continue;
                    }
                    "center" => {
                        let (env_content, new_i) = collect_environment(&lines, i, "center");
                        let content_trimmed = env_content.trim();
                        if content_trimmed.starts_with("\\rule{") || content_trimmed.starts_with("\\rule ") {
                            b.push_paragraph("---", vec![], None, None);
                        } else {
                            let inner_lines: Vec<&str> = env_content.lines().collect();
                            Self::build_internal_body(&mut b, &inner_lines, heading_map, inject_placeholders);
                        }
                        i = new_i;
                        continue;
                    }
                    _ => {
                        let (env_content, new_i) = collect_environment(&lines, i, &env_name);
                        let inner_lines: Vec<&str> = env_content.lines().collect();
                        Self::build_internal_body(&mut b, &inner_lines, heading_map, inject_placeholders);
                        i = new_i;
                        continue;
                    }
                }
            }

            Self::process_content_line(trimmed, &lines, &mut i, &mut b, heading_map, inject_placeholders);

            i += 1;
        }

        b.build()
    }

    /// Process body lines (shared between top-level and recursive calls for environments like quote).
    fn build_internal_body(
        b: &mut InternalDocumentBuilder,
        lines: &[&str],
        heading_map: &ahash::AHashMap<&'static str, u8>,
        inject_placeholders: bool,
    ) {
        let mut i = 0;
        while i < lines.len() {
            let trimmed = lines[i].trim();

            if (trimmed.contains("\\begin{") || trimmed.contains("\\begin {"))
                && let Some(env_name) = extract_env_name(trimmed)
            {
                match env_name.as_str() {
                    "itemize" | "enumerate" | "description" => {
                        let ordered = env_name == "enumerate";
                        let (env_content, new_i) = collect_environment(lines, i, &env_name);
                        b.push_list(ordered);
                        Self::build_internal_list_items(b, &env_content, ordered);
                        b.end_list();
                        i = new_i;
                        continue;
                    }
                    "tabular" | "longtable" | "tabularx" | "tabulary" => {
                        let (env_content, new_i) = collect_environment(lines, i, &env_name);
                        let cells = Self::parse_tabular_cells(&env_content);
                        if !cells.is_empty() {
                            let caption = Self::extract_caption(&env_content);
                            let label = Self::extract_label(&env_content);
                            let idx = b.push_table_from_cells(&cells, None, None);
                            if let Some(lbl) = label {
                                b.set_anchor(idx, &lbl);
                            }
                            if let Some(cap) = caption {
                                let cap_idx = b.push_paragraph(&cap, vec![], None, None);
                                b.push_relationship(cap_idx, RelationshipTarget::Index(idx), RelationshipKind::Caption);
                            }
                        }
                        i = new_i;
                        continue;
                    }
                    "equation" | "equation*" | "align" | "align*" | "gather" | "gather*" | "multline" | "multline*"
                    | "eqnarray" | "eqnarray*" | "math" | "displaymath" | "flalign" | "flalign*" | "cases" => {
                        let (env_content, new_i) = collect_environment(lines, i, &env_name);
                        let formula_text = format!("\\begin{{{}}}\n{}\\end{{{}}}", env_name, env_content, env_name);
                        b.push_formula(&formula_text, None, None);
                        i = new_i;
                        continue;
                    }
                    "lstlisting" | "verbatim" | "minted" | "Verbatim" => {
                        let (env_content, new_i) = collect_environment(lines, i, &env_name);
                        let language = if env_name == "lstlisting" || env_name == "minted" {
                            Self::extract_code_language(trimmed)
                        } else {
                            None
                        };
                        b.push_code(env_content.trim(), language, None, None);
                        i = new_i;
                        continue;
                    }
                    "quote" | "quotation" => {
                        let (env_content, new_i) = collect_environment(lines, i, &env_name);
                        b.push_quote_start();
                        let inner_lines: Vec<&str> = env_content.lines().collect();
                        Self::build_internal_body(b, &inner_lines, heading_map, inject_placeholders);
                        b.push_quote_end();
                        i = new_i;
                        continue;
                    }
                    "center" => {
                        let (env_content, new_i) = collect_environment(lines, i, "center");
                        let content_trimmed = env_content.trim();
                        if content_trimmed.starts_with("\\rule{") || content_trimmed.starts_with("\\rule ") {
                            b.push_paragraph("---", vec![], None, None);
                        } else {
                            let inner_lines: Vec<&str> = env_content.lines().collect();
                            Self::build_internal_body(b, &inner_lines, heading_map, inject_placeholders);
                        }
                        i = new_i;
                        continue;
                    }
                    _ => {
                        let (env_content, new_i) = collect_environment(lines, i, &env_name);
                        let inner_lines: Vec<&str> = env_content.lines().collect();
                        Self::build_internal_body(b, &inner_lines, heading_map, inject_placeholders);
                        i = new_i;
                        continue;
                    }
                }
            }

            Self::process_content_line(trimmed, lines, &mut i, b, heading_map, inject_placeholders);

            i += 1;
        }
    }

    /// Commands that should be silently skipped (no text output).
    const SKIP_COMMANDS: &[&str] = &[
        "maketitle",
        "tableofcontents",
        "listoffigures",
        "listoftables",
        "setcounter",
        "addtocounter",
        "newpage",
        "clearpage",
        "cleardoublepage",
        "pagestyle",
        "thispagestyle",
        "pagenumbering",
        "setlength",
        "addtolength",
        "newcommand",
        "renewcommand",
        "def",
        "let",
        "input",
        "include",
        "bibliography",
        "bibliographystyle",
        "graphicspath",
        "geometry",
        "hypersetup",
        "usepackage",
        "documentclass",
        "doublespacing",
        "singlespacing",
        "onehalfspacing",
        "VerbatimFootnotes",
    ];

    /// If `trimmed` is an `\input{file}` or `\include{file}` command, return the
    /// command name and the referenced file argument.
    ///
    /// Both commands are in [`Self::SKIP_COMMANDS`] because this extractor works
    /// line-by-line over a single file and has no way to resolve or read the
    /// referenced file — so the entire referenced document's content vanishes
    /// with no trace (#171). That is a real, guaranteed loss on every match, not
    /// a heuristic, so it is always worth naming.
    fn extract_input_include_target(trimmed: &str) -> Option<(&'static str, String)> {
        let after_backslash = trimmed.strip_prefix('\\')?;
        // Match the command-name boundary exactly as `is_skip_command` does, so
        // `\inputenc{...}` (a real, unrelated command) is never mistaken for
        // `\input{...}`.
        let cmd_end = after_backslash
            .find(|c: char| !c.is_alphabetic())
            .unwrap_or(after_backslash.len());
        let cmd = match &after_backslash[..cmd_end] {
            "input" => "input",
            "include" => "include",
            _ => return None,
        };
        let after = after_backslash[cmd_end..].trim_start();
        let target = if let Some(braced) = after.strip_prefix('{') {
            braced.split('}').next()?.trim()
        } else {
            after.trim()
        };
        if target.is_empty() {
            return None;
        }
        Some((cmd, target.to_string()))
    }

    /// Check if a line starts with a command that should be silently skipped.
    fn is_skip_command(trimmed: &str) -> bool {
        if !trimmed.starts_with('\\') {
            return false;
        }
        let after = &trimmed[1..];
        let cmd_end = after.find(|c: char| !c.is_alphabetic()).unwrap_or(after.len());
        let cmd = &after[..cmd_end];
        Self::SKIP_COMMANDS.contains(&cmd)
    }

    /// Process a single content line (heading, image, math, or paragraph).
    fn process_content_line(
        trimmed: &str,
        lines: &[&str],
        i: &mut usize,
        b: &mut InternalDocumentBuilder,
        heading_map: &ahash::AHashMap<&'static str, u8>,
        inject_placeholders: bool,
    ) {
        if trimmed.is_empty() || trimmed.starts_with('%') {
            return;
        }

        if let Some((cmd, target)) = Self::extract_input_include_target(trimmed) {
            b.add_warning(crate::core::diagnostics::warning(
                LATEX_WARNING_SOURCE,
                format!(
                    "\\{cmd}{{{target}}} references an external file that was not read; \
                     its content is missing from the extracted text"
                ),
            ));
            return;
        }

        if Self::is_skip_command(trimmed) {
            return;
        }

        if let Some(after_backslash) = trimmed.strip_prefix('\\') {
            let cmd_end = after_backslash
                .find(|c: char| c == '{' || c == '[' || c.is_whitespace())
                .unwrap_or(after_backslash.len());
            let cmd_name = &after_backslash[..cmd_end];
            if let Some(&level) = heading_map.get(cmd_name) {
                let rest = &after_backslash[cmd_end..].trim_start();
                if rest.starts_with('{') || rest.starts_with('[') {
                    if let Some(title) = extract_heading_title(trimmed, cmd_name) {
                        let (title_text, title_anns) = Self::strip_inline_commands(&title);
                        let idx = b.push_heading(level, &title_text, None, None);
                        if !title_anns.is_empty() {
                            for ann in &title_anns {
                                if let AnnotationKind::Link { url, .. } = &ann.kind
                                    && !url.is_empty()
                                {
                                    let label = title_text
                                        .get(ann.start as usize..ann.end as usize)
                                        .map(|s| s.to_string());
                                    b.push_uri(ExtractedUri::hyperlink(url, label));
                                }
                            }
                        }
                        if let Some(lbl) = Self::extract_label(trimmed) {
                            b.set_anchor(idx, &lbl);
                        }
                    }
                    return;
                }
            }
        }

        if trimmed.contains("\\includegraphics")
            && let Some(path) = Self::extract_includegraphics_path(trimmed)
        {
            b.push_uri(ExtractedUri::image(&path, None));
            if inject_placeholders {
                b.push_paragraph(&format!("[image: {}]", path), vec![], None, None);
            }
            return;
        }

        Self::extract_refs(trimmed, b, "\\ref{", RelationshipKind::CrossReference);
        Self::extract_refs(trimmed, b, "\\cite{", RelationshipKind::CitationReference);

        if trimmed.starts_with("\\[") {
            let mut math_content = trimmed.to_string();
            if !trimmed.contains("\\]") {
                *i += 1;
                while *i < lines.len() {
                    math_content.push('\n');
                    math_content.push_str(lines[*i]);
                    if lines[*i].trim().contains("\\]") {
                        break;
                    }
                    *i += 1;
                }
            }
            let formula = math_content.trim_start_matches("\\[").trim_end_matches("\\]").trim();
            if !formula.is_empty() {
                b.push_formula(formula, None, None);
            }
            return;
        }

        let mut line_text = trimmed.to_string();
        while let Some(fn_start) = line_text.find("\\footnote{") {
            let after = &line_text[fn_start + "\\footnote{".len()..];
            if let Some((fn_text, consumed)) = Self::read_braced_content(after) {
                let fn_stripped = utilities::clean_text(&fn_text);
                if !fn_stripped.is_empty() {
                    let fn_key = format!("fn:{}", fn_stripped.chars().take(20).collect::<String>());
                    b.push_footnote_ref(&fn_stripped, &fn_key, None);
                    b.push_footnote_definition(&fn_stripped, &fn_key, None);
                }
                let end = fn_start + "\\footnote{".len() + consumed;
                line_text = format!("{}{}", &line_text[..fn_start], &line_text[end..]);
            } else {
                break;
            }
        }

        let line_text = line_text.trim();
        if !line_text.is_empty() {
            let (text, annotations) = Self::strip_inline_commands(line_text);
            let text = text.trim();
            if !text.is_empty() {
                for ann in &annotations {
                    if let AnnotationKind::Link { url, .. } = &ann.kind
                        && !url.is_empty()
                    {
                        let label = text.get(ann.start as usize..ann.end as usize).map(|s| s.to_string());
                        b.push_uri(ExtractedUri::hyperlink(url, label));
                    }
                }
                let idx = b.push_paragraph(text, annotations, None, None);
                if let Some(lbl) = Self::extract_label(line_text) {
                    b.set_anchor(idx, &lbl);
                }
            }
        }
    }

    /// Extract `\label{key}` from text.
    fn extract_label(text: &str) -> Option<String> {
        let prefix = "\\label{";
        let start = text.find(prefix)?;
        let after = &text[start + prefix.len()..];
        Self::read_braced_content(after).map(|(content, _)| content)
    }

    /// Extract `\ref{key}` or `\cite{key}` references and emit relationships.
    fn extract_refs(text: &str, b: &mut InternalDocumentBuilder, prefix: &str, kind: RelationshipKind) {
        let mut search_from = 0;
        while let Some(pos) = text[search_from..].find(prefix) {
            let abs_pos = search_from + pos;
            let after = &text[abs_pos + prefix.len()..];
            if let Some((key, consumed)) = Self::read_braced_content(after) {
                let keys: Vec<&str> = key.split(',').map(|k| k.trim()).collect();
                for k in keys {
                    if !k.is_empty() {
                        let ref_text = format!("[{}]", k);
                        let idx = b.push_paragraph(&ref_text, vec![], None, None);
                        b.push_relationship(idx, RelationshipTarget::Key(k.to_string()), kind);
                    }
                }
                search_from = abs_pos + prefix.len() + consumed;
            } else {
                break;
            }
        }
    }

    /// Build list items for InternalDocument.
    fn build_internal_list_items(b: &mut InternalDocumentBuilder, content: &str, ordered: bool) {
        let all_lines: Vec<&str> = content.lines().collect();
        let mut i = 0;

        while i < all_lines.len() {
            let trimmed = all_lines[i].trim();

            if (trimmed.contains("\\begin{itemize}")
                || trimmed.contains("\\begin{enumerate}")
                || trimmed.contains("\\begin{description}"))
                && let Some(env_name) = extract_env_name(trimmed)
            {
                let nested_ordered = env_name == "enumerate";
                let (env_content, new_i) = collect_environment(&all_lines, i, &env_name);
                b.push_list(nested_ordered);
                Self::build_internal_list_items(b, &env_content, nested_ordered);
                b.end_list();
                i = new_i;
                continue;
            }

            if trimmed.starts_with("\\item") {
                let after = trimmed.strip_prefix("\\item").unwrap_or("").trim();

                let mut item_parts = Vec::new();
                let first_part = if after.starts_with('[') {
                    if let Some(bracket_end) = after.find(']') {
                        let label = &after[1..bracket_end];
                        let rest = after[bracket_end + 1..].trim();
                        if rest.is_empty() {
                            format!("{}:", label)
                        } else {
                            format!("{}: {}", label, rest)
                        }
                    } else {
                        after.to_string()
                    }
                } else {
                    after.to_string()
                };

                if !first_part.is_empty() {
                    item_parts.push(first_part);
                }

                i += 1;
                while i < all_lines.len() {
                    let next = all_lines[i].trim();
                    if next.is_empty()
                        || next.starts_with("\\item")
                        || next.starts_with("\\begin{")
                        || next.starts_with("\\end{")
                        || next.starts_with("\\setcounter")
                    {
                        break;
                    }
                    item_parts.push(next.to_string());
                    i += 1;
                }

                let text = item_parts.join(" ");
                if !text.is_empty() {
                    let (stripped, annotations) = Self::strip_inline_commands(&text);
                    let stripped = stripped.trim();
                    if !stripped.is_empty() {
                        b.push_list_item(stripped, ordered, annotations, None, None);
                    }
                }
                continue;
            }

            i += 1;
        }
    }

    /// Parse table cells from `tabular`, `longtable`, `tabularx`, or
    /// `tabulary` environment content.
    ///
    /// All four environments share the same `&`-separated cell / `\\`-terminated
    /// row grid; they differ only in their column-spec argument syntax (already
    /// stripped out by [`collect_environment`]) and in `longtable`'s extra
    /// page-break markers (`\endhead`, `\endfoot`, ...), which are skipped via
    /// [`utilities::is_table_structural_line`].
    fn parse_tabular_cells(content: &str) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if utilities::is_table_structural_line(trimmed) {
                continue;
            }
            let row_str = trimmed.replace("\\\\", "").replace("\\hline", "");
            let cells: Vec<String> = row_str
                .split('&')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !cells.is_empty() {
                rows.push(cells);
            }
        }
        rows
    }

    /// Extract language from code environment options.
    fn extract_code_language(begin_line: &str) -> Option<&str> {
        if let Some(lang_pos) = begin_line.find("language=") {
            let after = &begin_line[lang_pos + 9..];
            let end = after.find([',', ']', '}']).unwrap_or(after.len());
            let lang = after[..end].trim();
            if !lang.is_empty() {
                return Some(lang);
            }
        }
        if begin_line.contains("minted")
            && let Some(brace_start) = begin_line.rfind('{')
        {
            let after = &begin_line[brace_start + 1..];
            if let Some(brace_end) = after.find('}') {
                let lang = after[..brace_end].trim();
                if !lang.is_empty() && lang != "minted" {
                    return Some(lang);
                }
            }
        }
        None
    }
}

impl Default for LatexExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for LatexExtractor {
    fn name(&self) -> &str {
        "latex-extractor"
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
        "Native Rust LaTeX document extractor with metadata and table support"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for LatexExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "latex", size_bytes = content.len(), "extraction starting");
        let mut budget = SecurityBudget::from_config(config);
        budget.account_text(content.len())?;
        let inject_placeholders = config
            .images
            .as_ref()
            .map(|img| img.inject_placeholders)
            .unwrap_or(true);
        let latex_str = String::from_utf8_lossy(content).into_owned();
        let (_text, metadata, _tables) = Self::extract_from_latex(&latex_str);
        let mut doc = Self::build_internal_document(&latex_str, inject_placeholders);
        doc.mime_type = mime_type.to_string();
        doc.metadata = metadata;
        tracing::debug!(
            element_count = doc.elements.len(),
            format = "latex",
            "extraction complete"
        );
        Ok(doc)
    }

    async fn extract_path(
        &self,
        path: &std::path::Path,
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        crate::core::path_resolver::extract_file_with_image_resolution(self, path, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-latex", "text/x-tex"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_title_extraction() {
        let latex = r#"\title{Hello World}"#;
        let (_, metadata, _) = LatexExtractor::extract_from_latex(latex);
        assert_eq!(metadata.title.as_deref(), Some("Hello World"));
    }

    #[test]
    fn test_author_extraction() {
        let latex = r#"\author{John Doe}"#;
        let (_, metadata, _) = LatexExtractor::extract_from_latex(latex);
        assert!(metadata.created_by.is_some());
    }

    #[test]
    fn test_section_extraction() {
        let latex = r#"\begin{document}\section{Introduction}\end{document}"#;
        let (content, _, _) = LatexExtractor::extract_from_latex(latex);
        assert!(content.contains("Introduction"));
    }

    #[test]
    fn test_strip_inline_bold() {
        let (text, anns) = LatexExtractor::strip_inline_commands("hello \\textbf{world} end");
        assert_eq!(text, "hello world end");
        assert_eq!(anns.len(), 1);
        assert!(matches!(anns[0].kind, AnnotationKind::Bold));
        assert_eq!(&text[anns[0].start as usize..anns[0].end as usize], "world");
    }

    #[test]
    fn test_strip_inline_italic_variants() {
        let (text, anns) = LatexExtractor::strip_inline_commands("\\emph{a} and \\textit{b}");
        assert_eq!(text, "a and b");
        assert_eq!(anns.len(), 2);
        assert!(anns.iter().all(|a| matches!(a.kind, AnnotationKind::Italic)));
    }

    #[test]
    fn test_strip_inline_underline_code() {
        let (text, anns) = LatexExtractor::strip_inline_commands("\\underline{u} \\texttt{c}");
        assert_eq!(text, "u c");
        assert!(anns.iter().any(|a| matches!(a.kind, AnnotationKind::Underline)));
        assert!(anns.iter().any(|a| matches!(a.kind, AnnotationKind::Code)));
    }

    #[test]
    fn test_strip_inline_nested() {
        let (text, anns) = LatexExtractor::strip_inline_commands("\\textbf{\\emph{nested}}");
        assert_eq!(text, "nested");
        assert_eq!(anns.len(), 2);
        assert!(anns.iter().any(|a| matches!(a.kind, AnnotationKind::Bold)));
        assert!(anns.iter().any(|a| matches!(a.kind, AnnotationKind::Italic)));
    }

    #[test]
    fn test_strip_inline_href() {
        let (text, anns) = LatexExtractor::strip_inline_commands("see \\href{https://example.com}{link text} here");
        assert_eq!(text, "see link text here");
        assert_eq!(anns.len(), 1);
        match &anns[0].kind {
            AnnotationKind::Link { url, .. } => assert_eq!(url, "https://example.com"),
            _ => panic!("expected Link annotation"),
        }
        assert_eq!(&text[anns[0].start as usize..anns[0].end as usize], "link text");
    }

    #[test]
    fn test_strip_no_commands() {
        let (text, anns) = LatexExtractor::strip_inline_commands("plain text only");
        assert_eq!(text, "plain text only");
        assert!(anns.is_empty());
    }

    #[test]
    fn test_extract_includegraphics_path() {
        assert_eq!(
            LatexExtractor::extract_includegraphics_path("\\includegraphics[width=5cm]{img/photo.png}"),
            Some("img/photo.png".to_string())
        );
        assert_eq!(
            LatexExtractor::extract_includegraphics_path("\\includegraphics{simple.jpg}"),
            Some("simple.jpg".to_string())
        );
        assert_eq!(LatexExtractor::extract_includegraphics_path("no graphics here"), None);
    }

    #[test]
    fn test_extract_caption() {
        assert_eq!(
            LatexExtractor::extract_caption("\\caption{A nice figure}"),
            Some("A nice figure".to_string())
        );
        assert_eq!(LatexExtractor::extract_caption("no caption"), None);
    }

    #[test]
    fn test_read_braced_content_nested() {
        let (content, consumed) = LatexExtractor::read_braced_content("outer {inner} end}rest").unwrap();
        assert_eq!(content, "outer {inner} end");
        assert_eq!(&"outer {inner} end}rest"[consumed..], "rest");
    }

    #[test]
    fn test_latex_inject_placeholders_true() {
        let latex = r#"\documentclass{article}
\begin{document}
\begin{figure}
\includegraphics{photo.png}
\caption{A photo}
\end{figure}
\end{document}"#;
        let doc = LatexExtractor::build_internal_document(latex, true);
        let has_image = doc.elements.iter().any(|e| e.text.contains("[image:"));
        assert!(has_image, "expected image placeholder with inject_placeholders=true");
    }

    #[test]
    fn test_latex_inject_placeholders_false() {
        let latex = r#"\documentclass{article}
\begin{document}
\begin{figure}
\includegraphics{photo.png}
\caption{A photo}
\end{figure}
\end{document}"#;
        let doc = LatexExtractor::build_internal_document(latex, false);
        let has_image = doc.elements.iter().any(|e| e.text.contains("[image:"));
        assert!(
            !has_image,
            "expected no image placeholder with inject_placeholders=false"
        );
    }

    fn latex_warnings(doc: &InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.source == LATEX_WARNING_SOURCE)
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #171: `\input`/`\include` are silently skipped because this extractor
    /// only ever sees a single in-memory file; the referenced file's content
    /// never appears anywhere in the output.
    #[test]
    fn should_warn_when_latex_input_command_is_skipped() {
        let latex = r#"\documentclass{article}
\begin{document}
\input{chapter1}
\end{document}"#;
        let doc = LatexExtractor::build_internal_document(latex, true);

        let warnings = latex_warnings(&doc);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one latex warning, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("\\input{chapter1}") && warnings[0].contains("was not read"),
            "warning must name the skipped \\input target, got {warnings:?}"
        );
    }

    #[test]
    fn should_warn_when_latex_include_command_is_skipped() {
        let latex = r#"\documentclass{article}
\begin{document}
\include{appendix}
\end{document}"#;
        let doc = LatexExtractor::build_internal_document(latex, true);

        let warnings = latex_warnings(&doc);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one latex warning, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("\\include{appendix}"),
            "warning must name the skipped \\include target, got {warnings:?}"
        );
    }

    /// A document with no `\input`/`\include` must not warn.
    #[test]
    fn should_not_warn_for_latex_document_without_input_or_include() {
        let latex = r#"\documentclass{article}
\begin{document}
\section{Introduction}
Plain body text.
\end{document}"#;
        let doc = LatexExtractor::build_internal_document(latex, true);

        assert!(
            latex_warnings(&doc).is_empty(),
            "a document without \\input/\\include must not warn, got {:?}",
            latex_warnings(&doc)
        );
    }

    /// #171: a `figure` environment with no `\includegraphics` (a caption-only
    /// figure, a `\resizebox`'d graphic, a sub-figure wrapper, ...) previously
    /// vanished with no trace — the arm's only other output path is the image
    /// placeholder.
    #[test]
    fn should_warn_when_latex_figure_without_includegraphics_is_skipped() {
        let latex = r#"\documentclass{article}
\begin{document}
\begin{figure}
\caption{A figure with no graphic}
\end{figure}
\end{document}"#;
        let doc = LatexExtractor::build_internal_document(latex, true);

        let warnings = latex_warnings(&doc);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one latex warning, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("figure environment without \\includegraphics"),
            "warning must describe the skipped figure, got {warnings:?}"
        );
    }

    /// A figure that does contain `\includegraphics` must not warn.
    #[test]
    fn should_not_warn_for_latex_figure_with_includegraphics() {
        let latex = r#"\documentclass{article}
\begin{document}
\begin{figure}
\includegraphics{photo.png}
\caption{A photo}
\end{figure}
\end{document}"#;
        let doc = LatexExtractor::build_internal_document(latex, true);

        assert!(
            latex_warnings(&doc).is_empty(),
            "a figure with \\includegraphics must not warn, got {:?}",
            latex_warnings(&doc)
        );
    }
}
