//! Utility functions for LaTeX parsing.
//!
//! This module contains helper functions for text cleaning, brace extraction,
//! and other common operations used throughout the LaTeX parser.

/// Extracts content from within braces for a given command.
///
/// Example: `\title{Hello World}` with command "title" returns "Hello World"
pub(crate) fn extract_braced(text: &str, command: &str) -> Option<String> {
    let pattern = format!("\\{}{{", command);
    if let Some(start) = text.find(&pattern) {
        let after = &text[start + pattern.len()..];
        let mut depth = 1;
        let mut content = String::new();

        for ch in after.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    content.push(ch);
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(clean_text(&content));
                    }
                    content.push(ch);
                }
                _ => content.push(ch),
            }
        }
    }
    None
}

/// Reads braced content from a character iterator.
///
/// Handles nested braces correctly and maintains proper depth tracking.
pub(crate) fn read_braced_from_chars(chars: &mut std::iter::Peekable<std::str::Chars>) -> Option<String> {
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }

    if chars.peek() != Some(&'{') {
        return None;
    }
    chars.next();

    let mut content = String::new();
    let mut depth = 1;

    for c in chars.by_ref() {
        match c {
            '{' => {
                depth += 1;
                content.push(c);
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(content);
                }
                content.push(c);
            }
            _ => content.push(c),
        }
    }

    Some(content)
}

/// Extracts environment name from a \begin{} statement.
///
/// Example: `\begin{itemize}` returns "itemize"
/// Also handles `\begin {itemize}` (with space).
pub(crate) fn extract_env_name(line: &str) -> Option<String> {
    let start = line.find("\\begin{").or_else(|| line.find("\\begin {"))?;
    let brace_pos = line[start..].find('{')?;
    let after = &line[start + brace_pos + 1..];
    let end = after.find('}')?;
    Some(after[..end].to_string())
}

/// Cleans LaTeX text by removing escape sequences.
///
/// Handles common LaTeX escape sequences like \\&, \\#, \\\_, etc.
pub(crate) fn clean_text(text: &str) -> String {
    text.to_string()
        .replace("\\\\", "\n")
        .replace("\\&", "&")
        .replace("\\#", "#")
        .replace("\\_", "_")
        .replace("\\{", "{")
        .replace("\\}", "}")
        .replace("\\%", "%")
        .trim()
        .to_string()
}

/// Returns true if a line inside a table-like environment (`tabular`,
/// `longtable`, `tabularx`, `tabulary`) is structural row/column markup
/// rather than literal cell content.
///
/// Covers `\hline`/booktabs rules, `longtable` page-break markers
/// (`\endhead`, `\endfirsthead`, `\endfoot`, `\endlastfoot`), `\caption{}`
/// and `\label{}` (handled separately by callers that want them), and any
/// nested `\begin{...}`/`\end{...}` boundary line.
pub(crate) fn is_table_structural_line(trimmed: &str) -> bool {
    trimmed.is_empty()
        || trimmed.starts_with("\\hline")
        || trimmed.starts_with("\\toprule")
        || trimmed.starts_with("\\midrule")
        || trimmed.starts_with("\\bottomrule")
        || trimmed.starts_with("\\endhead")
        || trimmed.starts_with("\\endfirsthead")
        || trimmed.starts_with("\\endfoot")
        || trimmed.starts_with("\\endlastfoot")
        || trimmed.starts_with("\\caption")
        || trimmed.starts_with("\\label")
        || trimmed.starts_with("\\begin{")
        || trimmed.starts_with("\\end{")
}

/// Collects content of an environment from begin to end.
///
/// Returns the content and the index of the line after \end{environment}.
/// Handles nested environments of the same type and single-line environments.
pub(crate) fn collect_environment(lines: &[&str], start_idx: usize, env_name: &str) -> (String, usize) {
    let end_marker = format!("\\end{{{}}}", env_name);
    let begin_marker = format!("\\begin{{{}}}", env_name);

    let start_line = lines[start_idx];
    if let Some(begin_pos) = start_line.find(&begin_marker) {
        let after_begin = &start_line[begin_pos + begin_marker.len()..];
        if let Some(end_pos) = after_begin.find(&end_marker) {
            let inner = &after_begin[..end_pos];
            return (inner.to_string(), start_idx + 1);
        }
    }

    let end_marker_space = format!("\\end {{{}}}", env_name);
    let begin_marker_space = format!("\\begin {{{}}}", env_name);

    let mut content = String::new();
    let mut i = start_idx + 1;
    let mut depth: isize = 1;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        depth += trimmed.matches(&begin_marker).count() as isize;
        depth += trimmed.matches(&begin_marker_space).count() as isize;
        depth -= trimmed.matches(&end_marker).count() as isize;
        depth -= trimmed.matches(&end_marker_space).count() as isize;

        if depth <= 0 {
            return (content, i + 1);
        }

        content.push_str(line);
        content.push('\n');
        i += 1;
    }

    (content, i)
}

/// Extracts a section/heading title, handling optional `[short]` arguments.
///
/// Supports `\section{title}`, `\section[short]{title}`, `\section*{title}`, etc.
pub(crate) fn extract_heading_title(line: &str, command: &str) -> Option<String> {
    let prefix = format!("\\{}", command);
    let start = line.find(&prefix)?;
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

    let content = &rest[1..];
    let mut depth = 1;
    let mut result = String::new();

    for ch in content.chars() {
        match ch {
            '{' => {
                depth += 1;
                result.push(ch);
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(clean_text(&result));
                }
                result.push(ch);
            }
            _ => result.push(ch),
        }
    }
    None
}
