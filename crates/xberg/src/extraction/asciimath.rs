//! AsciiMath to LaTeX.
//!
//! AsciiDoc writes `stem:[]` content in AsciiMath unless the document sets
//! `:stem: latexmath`. `Formula.latex` holds LaTeX, so the notation has to be
//! converted.
//!
//! The conversion goes through MathML rather than a second hand-written
//! mapping: `mathemascii` (the AsciiMath parser) renders MathML, and the shared
//! MathML converter already turns that into LaTeX. That converter carries the
//! accent, fence, and escaping fixes the render sweep found across 26,504
//! formulas, so AsciiMath inherits all of them.

use crate::extractors::security::SecurityBudget;

/// Convert one AsciiMath expression to LaTeX.
///
/// Returns `None` when the expression yields no math, so the caller can keep
/// the source text rather than emit an empty formula.
pub(crate) fn convert_asciimath_to_latex(source: &str, budget: &mut SecurityBudget) -> Option<String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return None;
    }
    // `mathemascii` 0.4.0 scans by byte index and splits a multi-byte character,
    // so a real specification that writes `≤` panics inside the parser and takes
    // the whole extraction with it. The panic is contained here, the way the
    // crate already contains ORT and pdf_oxide. The equation itself is lost,
    // because there is no LaTeX to report, and the document survives.
    let mathml = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        mathemascii::render_mathml(mathemascii::parse(trimmed))
    }))
    .map_err(|_| {
        log::debug!("asciimath parser panicked, dropping this equation");
    })
    .ok()?;
    let latex = crate::extraction::mathml::convert_mathml_str_to_latex(&mathml, budget).ok()?;
    let latex = latex.trim();
    if latex.is_empty() { None } else { Some(latex.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(source: &str) -> Option<String> {
        convert_asciimath_to_latex(source, &mut SecurityBudget::with_defaults())
    }

    /// A published specification writes `≤` in its AsciiMath. The parser slices
    /// by byte index, so this input panicked and lost the whole document.
    #[test]
    fn test_multibyte_asciimath_does_not_panic() {
        // Reaching the next line at all is the test: this input aborted the
        // process before, so the whole document was lost.
        let _ = convert("t(t_i≤t≤t_(i+1))");
        assert!(convert("a+b").is_some(), "the converter still works afterwards");
    }

    /// A real specification writes AsciiMath like this. Each of these comes
    /// from a published document and must convert.
    #[test]
    fn test_real_specification_asciimath() {
        for source in [
            "b_(g0)=1/(mn)sum_(i=0)^(mn-1)g_i",
            "|barg_(xii)|leC",
            "|sigma_(xii)|lesigma",
        ] {
            let latex = convert(source);
            assert!(latex.is_some(), "no LaTeX for {source:?}");
            assert!(
                !latex.as_deref().unwrap_or("").trim().is_empty(),
                "empty LaTeX for {source:?}"
            );
        }
    }

    #[test]
    fn test_roots_and_fractions() {
        assert_eq!(convert("sqrt(4) = 2").as_deref(), Some("\\sqrt{4}=2"));
        assert_eq!(convert("a/b").as_deref(), Some("\\frac{a}{b}"));
    }

    #[test]
    fn test_sums_carry_their_limits() {
        let latex = convert("sum_(i=1)^n i^3").expect("converts");
        assert!(latex.contains("\\sum"), "got: {latex}");
        assert!(latex.contains("i=1"), "the lower limit survives: {latex}");
    }

    #[test]
    fn test_quadratic_formula() {
        let latex = convert("x = (-b +- sqrt(b^2-4ac))/(2a)").expect("converts");
        assert!(latex.contains("\\frac"), "got: {latex}");
        assert!(latex.contains("\\pm"), "the plus-minus symbol maps: {latex}");
    }

    #[test]
    fn test_empty_input_yields_nothing() {
        assert_eq!(convert("   "), None);
    }
}
