//! LaTeX environment processing.
//!
//! This module handles LaTeX environments like itemize, enumerate, description,
//! tabular, and table environments.

use super::commands::process_line;
use super::utilities::{clean_text, collect_environment, extract_braced, extract_env_name, is_table_structural_line};
use crate::rendering::common::render_table_markdown;
use crate::types::Table;

/// Processes a list environment (itemize, enumerate, or description).
///
/// Converts LaTeX lists into markdown-style lists with proper nesting.
pub(crate) fn process_list(content: &str, list_type: &str, output: &mut String) {
    let lines: Vec<&str> = content.lines().collect();
    let mut item_num = 1;
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if (trimmed.contains("\\begin{") || trimmed.contains("\\begin {"))
            && let Some(env_name) = extract_env_name(trimmed)
            && (env_name == "itemize" || env_name == "enumerate" || env_name == "description")
        {
            let (nested_content, new_i) = collect_environment(&lines, i, &env_name);
            let current_output_len = output.len();
            process_list(&nested_content, &env_name, output);
            let nested_output = output[current_output_len..].to_string();
            output.truncate(current_output_len);
            for nested_line in nested_output.lines() {
                output.push_str("  ");
                output.push_str(nested_line);
                output.push('\n');
            }
            i = new_i;
            continue;
        }

        if trimmed.starts_with("\\item")
            && let Some(pos) = trimmed.find("\\item")
        {
            let after = trimmed[pos + 5..].trim();

            if after.starts_with('[')
                && let Some(bracket_end) = after.find(']')
            {
                let label = after[1..bracket_end].to_string();
                let text = after[bracket_end + 1..].trim().to_string();
                if list_type == "description" {
                    let processed_text = process_line(&text);
                    output.push_str(&format!("{}: {}\n", label, processed_text));
                    item_num += 1;
                    i += 1;
                    continue;
                }
            }

            let prefix = if list_type == "enumerate" {
                format!("{}. ", item_num)
            } else {
                "- ".to_string()
            };
            output.push_str(&prefix);

            let item_text = process_line(after);
            output.push_str(item_text.trim());
            output.push('\n');
            item_num += 1;
        }

        i += 1;
    }
    output.push('\n');
}

/// Processes a `tabular`, `longtable`, `tabularx`, or `tabulary` environment.
///
/// All four share the same `&`-separated cell / `\\`-terminated row grid; the
/// column-spec argument syntax that differs between them is already stripped
/// out by [`collect_environment`], and `longtable`'s page-break markers
/// (`\endhead`, `\endfoot`, ...) are skipped via [`is_table_structural_line`].
///
/// Converts LaTeX tables into markdown tables (via the crate's canonical
/// [`render_table_markdown`]) and creates `Table` structures.
pub(crate) fn process_table(content: &str, output: &mut String, tables: &mut Vec<Table>) {
    let lines: Vec<&str> = content.lines().collect();
    let mut rows: Vec<Vec<String>> = Vec::new();

    for line in lines {
        let trimmed = line.trim();
        if is_table_structural_line(trimmed) {
            continue;
        }

        let row_str = trimmed.replace("\\\\", "");
        let cells: Vec<String> = row_str
            .split('&')
            .map(|s| clean_text(s.trim()))
            .filter(|s| !s.is_empty())
            .collect();

        if !cells.is_empty() {
            rows.push(cells);
        }
    }

    if !rows.is_empty() {
        let markdown = render_table_markdown(&rows);
        output.push_str(&markdown);

        let table = Table {
            cells: rows,
            markdown: markdown.clone(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        };
        tables.push(table);
    }
}

/// Processes a table environment with caption.
///
/// Extracts the caption and processes the embedded tabular environment.
pub(crate) fn process_table_with_caption(content: &str, output: &mut String, tables: &mut Vec<Table>) {
    if content.contains("\\caption{")
        && let Some(caption) = extract_braced(content, "caption")
    {
        output.push_str(&caption);
        output.push('\n');
    }

    let end_tag = "\\end{tabular}";
    if content.contains("\\begin{tabular}")
        && let Some(start) = content.find("\\begin{tabular}")
        && let Some(end) = content.find(end_tag)
    {
        let tabular_content = &content[start..end + end_tag.len()];
        process_table(tabular_content, output, tables);
    }
}
