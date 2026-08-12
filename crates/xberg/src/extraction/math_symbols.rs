//! Shared Unicode-to-LaTeX symbol mapping for math converters.
//!
//! Both the OMML (`docx::math`) and MathML (`mathml`) converters render run/text
//! nodes through the same table of Unicode math symbols (Greek letters, operators,
//! arrows, set notation, etc.) so a given symbol always maps to the same LaTeX
//! command regardless of which source format it came from.

/// Render run text, mapping Unicode math symbols to LaTeX commands and
/// escaping TeX structural characters.
///
/// Source text can carry literal `{`, `}`, `&`, `\` and friends (e.g. a
/// stretchy `<mo>{</mo>` cases brace, or a set-difference backslash in
/// `<mi>`). Passed through raw they change the LaTeX *structure* — an
/// unpaired brace makes the whole formula unparseable. Escaped they render
/// as the glyphs the source displayed.
pub(crate) fn render_run_text(text: &str, out: &mut String) {
    for ch in text.chars() {
        if let Some(latex) = unicode_to_latex(ch) {
            out.push_str(latex);
        } else if let Some(escaped) = escape_tex_structural(ch) {
            out.push_str(escaped);
        } else {
            out.push(ch);
        }
    }
}

/// Escape a TeX structural character appearing as literal content.
pub(crate) fn escape_tex_structural(ch: char) -> Option<&'static str> {
    match ch {
        '{' => Some("\\{"),
        '}' => Some("\\}"),
        '&' => Some("\\&"),
        '%' => Some("\\%"),
        '#' => Some("\\#"),
        '$' => Some("\\$"),
        '_' => Some("\\_"),
        '\\' => Some("\\backslash "),
        _ => None,
    }
}

/// Map a Unicode character to its LaTeX command (if any).
pub(crate) fn unicode_to_latex(ch: char) -> Option<&'static str> {
    match ch {
        '\u{03B1}' => Some("\\alpha "),
        '\u{03B2}' => Some("\\beta "),
        '\u{03B3}' => Some("\\gamma "),
        '\u{03B4}' => Some("\\delta "),
        '\u{03B5}' => Some("\\epsilon "),
        '\u{03B6}' => Some("\\zeta "),
        '\u{03B7}' => Some("\\eta "),
        '\u{03B8}' => Some("\\theta "),
        '\u{03B9}' => Some("\\iota "),
        '\u{03BA}' => Some("\\kappa "),
        '\u{03BB}' => Some("\\lambda "),
        '\u{03BC}' => Some("\\mu "),
        '\u{03BD}' => Some("\\nu "),
        '\u{03BE}' => Some("\\xi "),
        '\u{03BF}' => Some("o"),
        '\u{03C0}' => Some("\\pi "),
        '\u{03C1}' => Some("\\rho "),
        '\u{03C2}' => Some("\\varsigma "),
        '\u{03C3}' => Some("\\sigma "),
        '\u{03C4}' => Some("\\tau "),
        '\u{03C5}' => Some("\\upsilon "),
        '\u{03C6}' => Some("\\phi "),
        '\u{03C7}' => Some("\\chi "),
        '\u{03C8}' => Some("\\psi "),
        '\u{03C9}' => Some("\\omega "),
        '\u{0393}' => Some("\\Gamma "),
        '\u{0394}' => Some("\\Delta "),
        '\u{0398}' => Some("\\Theta "),
        '\u{039B}' => Some("\\Lambda "),
        '\u{039E}' => Some("\\Xi "),
        '\u{03A0}' => Some("\\Pi "),
        '\u{03A3}' => Some("\\Sigma "),
        '\u{03A5}' => Some("\\Upsilon "),
        '\u{03A6}' => Some("\\Phi "),
        '\u{03A8}' => Some("\\Psi "),
        '\u{03A9}' => Some("\\Omega "),
        '\u{00B1}' => Some("\\pm "),
        '\u{2213}' => Some("\\mp "),
        '\u{00D7}' => Some("\\times "),
        '\u{00F7}' => Some("\\div "),
        '\u{22C5}' => Some("\\cdot "),
        '\u{2217}' => Some("\\ast "),
        '\u{2218}' => Some("\\circ "),
        '\u{2219}' => Some("\\bullet "),
        '\u{2264}' => Some("\\leq "),
        '\u{2265}' => Some("\\geq "),
        '\u{2260}' => Some("\\neq "),
        '\u{2248}' => Some("\\approx "),
        '\u{2261}' => Some("\\equiv "),
        '\u{227A}' => Some("\\prec "),
        '\u{227B}' => Some("\\succ "),
        '\u{2286}' => Some("\\subseteq "),
        '\u{2287}' => Some("\\supseteq "),
        '\u{2282}' => Some("\\subset "),
        '\u{2283}' => Some("\\supset "),
        '\u{2208}' => Some("\\in "),
        '\u{2209}' => Some("\\notin "),
        '\u{220B}' => Some("\\ni "),
        '\u{2190}' => Some("\\leftarrow "),
        '\u{2192}' => Some("\\rightarrow "),
        '\u{2191}' => Some("\\uparrow "),
        '\u{2193}' => Some("\\downarrow "),
        '\u{2194}' => Some("\\leftrightarrow "),
        '\u{21D0}' => Some("\\Leftarrow "),
        '\u{21D2}' => Some("\\Rightarrow "),
        '\u{21D4}' => Some("\\Leftrightarrow "),
        '\u{21A6}' => Some("\\mapsto "),
        '\u{221E}' => Some("\\infty "),
        '\u{2202}' => Some("\\partial "),
        '\u{2207}' => Some("\\nabla "),
        '\u{2200}' => Some("\\forall "),
        '\u{2203}' => Some("\\exists "),
        '\u{2205}' => Some("\\emptyset "),
        '\u{2227}' => Some("\\wedge "),
        '\u{2228}' => Some("\\vee "),
        '\u{00AC}' => Some("\\neg "),
        '\u{2229}' => Some("\\cap "),
        '\u{222A}' => Some("\\cup "),
        '\u{2026}' => Some("\\ldots "),
        '\u{22EF}' => Some("\\cdots "),
        '\u{22EE}' => Some("\\vdots "),
        '\u{22F1}' => Some("\\ddots "),
        '\u{2032}' => Some("'"),
        '\u{2033}' => Some("''"),
        '\u{210F}' => Some("\\hbar "),
        '\u{2113}' => Some("\\ell "),
        '\u{211C}' => Some("\\Re "),
        '\u{2111}' => Some("\\Im "),
        '\u{2118}' => Some("\\wp "),
        '\u{2135}' => Some("\\aleph "),
        '\u{2211}' => Some("\\sum "),
        '\u{220F}' => Some("\\prod "),
        '\u{222B}' => Some("\\int "),
        '\u{222C}' => Some("\\iint "),
        '\u{222D}' => Some("\\iiint "),
        '\u{222E}' => Some("\\oint "),
        '\u{2210}' => Some("\\coprod "),
        '\u{22C0}' => Some("\\bigwedge "),
        '\u{22C1}' => Some("\\bigvee "),
        '\u{22C2}' => Some("\\bigcap "),
        '\u{22C3}' => Some("\\bigcup "),
        _ => None,
    }
}
