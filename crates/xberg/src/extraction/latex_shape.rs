//! Shaping helpers for LaTeX that an extractor has produced.

/// Wrap content that uses alignment points in an `aligned` environment.
///
/// A bare `&` outside an environment is a LaTeX error, so math that carries
/// alignment needs the environment around it. Content that already opens its
/// own environment keeps it.
pub(crate) fn wrap_aligned_math(content: &str) -> String {
    let has_alignment = content.contains('&') && !content.contains("\\&");
    if has_alignment && !content.contains("\\begin{") {
        format!("\\begin{{aligned}}{content}\\end{{aligned}}")
    } else {
        content.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alignment_gets_an_environment() {
        assert_eq!(
            wrap_aligned_math("a &= b"),
            "\\begin{aligned}a &= b\\end{aligned}"
        );
    }

    #[test]
    fn test_escaped_ampersand_is_not_alignment() {
        assert_eq!(wrap_aligned_math("a \\& b"), "a \\& b");
    }

    #[test]
    fn test_existing_environment_is_kept() {
        let cases = "\\begin{cases}a &1\\end{cases}";
        assert_eq!(wrap_aligned_math(cases), cases);
    }

    #[test]
    fn test_plain_math_is_unchanged() {
        assert_eq!(wrap_aligned_math("x^2"), "x^2");
    }
}
