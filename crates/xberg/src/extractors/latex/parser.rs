//! Core LaTeX parser implementation.
//!
//! This module contains the main LatexParser struct and the core parsing logic
//! that orchestrates document structure extraction.

use super::commands::process_line;
use super::environments::{process_list, process_table, process_table_with_caption};
use super::metadata::extract_metadata_from_line;
use super::utilities::{collect_environment, extract_braced, extract_env_name, extract_heading_title};
use crate::types::{Metadata, Table};

/// LaTeX parser state machine.
///
/// Maintains parsing state including metadata, tables, and output as it
/// processes a LaTeX document line by line.
pub struct LatexParser<'a> {
    source: &'a str,
    metadata: Metadata,
    tables: Vec<Table>,
    output: String,
}

impl<'a> LatexParser<'a> {
    /// Creates a new LaTeX parser for the given source.
    pub(crate) fn new(source: &'a str) -> Self {
        Self {
            source,
            metadata: Metadata::default(),
            tables: Vec::new(),
            output: String::new(),
        }
    }

    /// Parses the LaTeX document and returns extracted content, metadata, and tables.
    pub(crate) fn parse(&mut self) -> (String, Metadata, Vec<Table>) {
        let lines: Vec<&str> = self.source.lines().collect();
        let mut in_document = false;
        let mut i = 0;

        let is_plain_tex = self.source.contains("\\bye") && !self.source.contains("\\begin{document}");
        if is_plain_tex {
            in_document = true;
        }

        while i < lines.len() {
            let line = lines[i];
            let trimmed = line.trim();

            if is_plain_tex && trimmed.contains("\\bye") {
                break;
            }

            if !in_document && !is_plain_tex {
                extract_metadata_from_line(trimmed, &mut self.metadata);
            }

            if !is_plain_tex && trimmed.contains("\\begin{document}") {
                in_document = true;

                if trimmed.contains("\\end{document}") {
                    self.process_single_line_document(trimmed);
                    break;
                }

                i += 1;
                continue;
            }

            if !is_plain_tex && trimmed.contains("\\end{document}") {
                break;
            }

            if in_document {
                if self.process_environments(&lines, trimmed, &mut i) {
                    continue;
                }

                self.process_sections_and_content(trimmed, &lines, &mut i);
            }

            i += 1;
        }

        let content = self.output.trim().to_string();
        (content, self.metadata.clone(), self.tables.clone())
    }

    /// Processes a single-line document (both \begin and \end on same line).
    fn process_single_line_document(&mut self, trimmed: &str) {
        let begin_tag = "\\begin{document}";
        let end_tag = "\\end{document}";
        let Some(begin_pos) = trimmed.find(begin_tag) else {
            return;
        };
        let Some(end_pos) = trimmed.find(end_tag) else {
            return;
        };
        let content_between = trimmed[begin_pos + begin_tag.len()..end_pos].trim();
        if !content_between.is_empty() {
            let lines = [content_between];
            let mut i = 0;
            self.process_sections_and_content(content_between, &lines, &mut i);
        }
    }

    /// Processes LaTeX environments (lists, tables, math).
    ///
    /// Returns true if an environment was processed and the line index was updated.
    fn process_environments(&mut self, lines: &[&str], trimmed: &str, i: &mut usize) -> bool {
        if !trimmed.contains("\\begin{") && !trimmed.contains("\\begin {") {
            return false;
        }

        let Some(env_name) = extract_env_name(trimmed) else {
            return false;
        };

        match env_name.as_str() {
            "itemize" | "enumerate" | "description" => {
                let (env_content, new_i) = collect_environment(lines, *i, &env_name);
                process_list(&env_content, &env_name, &mut self.output);
                *i = new_i;
                true
            }
            "tabular" | "longtable" | "tabularx" | "tabulary" => {
                let (env_content, new_i) = collect_environment(lines, *i, &env_name);
                process_table(&env_content, &mut self.output, &mut self.tables);
                *i = new_i;
                true
            }
            "table" => {
                let (env_content, new_i) = collect_environment(lines, *i, "table");
                process_table_with_caption(&env_content, &mut self.output, &mut self.tables);
                *i = new_i;
                true
            }
            "equation" | "equation*" | "align" | "align*" | "gather" | "gather*" | "multline" | "multline*"
            | "eqnarray" | "eqnarray*" | "math" | "displaymath" | "flalign" | "flalign*" | "cases" => {
                let (env_content, new_i) = collect_environment(lines, *i, &env_name);
                self.output.push_str("$$\\begin{");
                self.output.push_str(&env_name);
                self.output.push_str("}\n");
                self.output.push_str(&env_content);
                self.output.push_str("\\end{");
                self.output.push_str(&env_name);
                self.output.push_str("}$$\n\n");
                *i = new_i;
                true
            }
            _ => {
                let (env_content, new_i) = collect_environment(lines, *i, &env_name);
                for line in env_content.lines() {
                    let trimmed_line = line.trim();
                    if trimmed_line.is_empty() || trimmed_line.starts_with('%') {
                        continue;
                    }
                    if trimmed_line.contains("\\begin{")
                        || trimmed_line.contains("\\begin {")
                        || trimmed_line.contains("\\end{")
                        || trimmed_line.contains("\\end {")
                    {
                        continue;
                    }
                    if trimmed_line.contains("\\caption{") {
                        if let Some(caption) = extract_braced(trimmed_line, "caption") {
                            self.output.push_str(&process_line(&caption));
                            self.output.push('\n');
                        }
                        continue;
                    }
                    let processed = process_line(trimmed_line);
                    if !processed.is_empty() {
                        self.output.push_str(&processed);
                        self.output.push('\n');
                    }
                }
                *i = new_i;
                true
            }
        }
    }

    /// Processes section headings, display math, and regular content.
    fn process_sections_and_content(&mut self, trimmed: &str, lines: &[&str], i: &mut usize) {
        let heading_commands = [
            ("chapter*", "\n# "),
            ("chapter", "\n# "),
            ("section*", "\n# "),
            ("section", "\n# "),
            ("subsection*", "## "),
            ("subsection", "## "),
            ("subsubsection*", "### "),
            ("subsubsection", "### "),
            ("paragraph*", "#### "),
            ("paragraph", "#### "),
        ];

        for (cmd, prefix) in heading_commands {
            let cmd_prefix = format!("\\{}", cmd);
            if trimmed.starts_with(&cmd_prefix) {
                let rest = &trimmed[cmd_prefix.len()..];
                if rest.starts_with('{') || rest.starts_with('[') {
                    if let Some(title) = extract_heading_title(trimmed, cmd) {
                        let processed = process_line(&title);
                        self.output.push_str(prefix);
                        self.output.push_str(&processed);
                        self.output.push_str("\n\n");
                    }
                    return;
                }
            }
        }

        if trimmed.starts_with("\\[") {
            self.process_display_math(trimmed, lines, i);
        } else if !trimmed.is_empty() && !trimmed.starts_with("%") {
            let processed = process_line(trimmed);
            if !processed.is_empty() {
                self.output.push_str(&processed);
                self.output.push('\n');
            }
        }
    }

    /// Processes display math mode \[...\].
    ///
    /// Converts `\[...\]` into `$$...$$` format for consistent output.
    fn process_display_math(&mut self, trimmed: &str, lines: &[&str], i: &mut usize) {
        let mut math_content = trimmed.to_string();
        if !trimmed.contains("\\]") {
            *i += 1;
            while *i < lines.len() {
                let math_line = lines[*i];
                math_content.push('\n');
                math_content.push_str(math_line);
                if math_line.trim().contains("\\]") {
                    break;
                }
                *i += 1;
            }
        }
        let converted = math_content.trim_start_matches("\\[").trim_end_matches("\\]").trim();
        self.output.push_str("$$");
        self.output.push_str(converted);
        self.output.push_str("$$\n");
    }
}
