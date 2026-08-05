//! CLI color styling helpers using `anstyle`.
//!
//! Provides styled output for the xberg CLI. Respects the `NO_COLOR`
//! environment variable (<https://no-color.org/>) and disables colors
//! when output is not a terminal.

use anstyle::{AnsiColor, Effects, Style};
use std::sync::OnceLock;

/// Bold blue for section headers.
const HEADER: Style = Style::new()
    .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Blue)))
    .effects(Effects::BOLD);

/// Green for success values (MIME types, file paths, versions).
const SUCCESS: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green)));

/// Yellow for warnings: actionable but not broken.
const WARNING: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow)));

/// Bold red for failures.
const ERROR: Style = Style::new()
    .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Red)))
    .effects(Effects::BOLD);

/// Dim for metadata, separators, secondary info.
const DIM: Style = Style::new().effects(Effects::DIMMED);

/// Bold for labels in key-value pairs.
const LABEL: Style = Style::new().effects(Effects::BOLD);

/// Check whether color output is enabled.
///
/// Returns `false` if:
/// - The `NO_COLOR` environment variable is set (any value)
///
/// See <https://no-color.org/> for the specification.
pub fn is_color_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("NO_COLOR").is_none())
}

/// Apply an `anstyle::Style` to text if colors are enabled.
fn styled(text: &str, style: Style) -> String {
    if is_color_enabled() {
        format!("{}{}{}", style.render(), text, style.render_reset())
    } else {
        text.to_string()
    }
}

/// Style text as a section header (bold blue).
pub fn header(text: &str) -> String {
    styled(text, HEADER)
}

/// Style text as a success value (green).
pub fn success(text: &str) -> String {
    styled(text, SUCCESS)
}

/// Style text as a warning (yellow).
pub fn warning(text: &str) -> String {
    styled(text, WARNING)
}

/// Style text as a failure (bold red).
pub fn error(text: &str) -> String {
    styled(text, ERROR)
}

/// Style text as dim/secondary (dimmed).
pub fn dim(text: &str) -> String {
    styled(text, DIM)
}

/// Style text as a label (bold).
pub fn label(text: &str) -> String {
    styled(text, LABEL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_styled_returns_plain_text_when_no_color() {
        let text = "hello";
        let result = format!("{}{}{}", Style::new().render(), text, Style::new().render_reset());
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_styled_applies_ansi_when_style_present() {
        let style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green)));
        let rendered = format!("{}{}{}", style.render(), "ok", style.render_reset());
        assert!(rendered.contains("\x1b["));
        assert!(rendered.contains("ok"));
    }

    #[test]
    fn test_helper_functions_return_strings() {
        assert!(!header("h").is_empty());
        assert!(!success("s").is_empty());
        assert!(!dim("d").is_empty());
        assert!(!label("l").is_empty());
    }

    #[test]
    fn test_is_color_enabled_respects_no_color_env() {
        let has_no_color = std::env::var_os("NO_COLOR").is_some();
        assert_eq!(is_color_enabled(), !has_no_color);
    }
}
