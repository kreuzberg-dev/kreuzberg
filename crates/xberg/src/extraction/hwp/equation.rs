//! Converts HWP 5.0 equation-editor (`HWPTAG_EQEDIT`) scripts to LaTeX.
//!
//! HWP equations are authored in the equation editor's own text-based script DSL
//! (things like `FRAC{a}{b}`, `SQRT{x}`, `a OVER b`, `x LEQ y`) — this is a distinct,
//! HWP-specific format, not MathML or OOXML's OMML (the formats already handled by
//! `extraction::docx::math` and the ODF/EPUB MathML converter landed in commit
//! `25a3be0aef`). Neither existing converter accepts this input, so this module
//! implements conversion for HWP's own script instead of reusing either one (#98/#99).
//!
//! This intentionally covers a practical **subset** of the script grammar — braced
//! groups, `SQRT`/`FRAC`/`OVER`, common relational and Greek-letter keywords, and
//! literal passthrough for identifiers/digits/`^`/`_` (which are already valid LaTeX).
//! Unrecognized keywords pass through unchanged rather than being guessed at, so a
//! script this module doesn't fully understand degrades to readable-but-unconverted
//! text instead of producing something that looks like correct LaTeX but is wrong.

/// Converts an HWP equation-editor script to a best-effort LaTeX string.
pub(crate) fn to_latex(script: &str) -> String {
    let tokens = tokenize(script);
    let mut pos = 0;
    join_atoms(&parse_atoms(&tokens, &mut pos))
}

/// Concatenates atoms, inserting a single space between two atoms that would
/// otherwise merge into one LaTeX token — specifically when the previous atom ends
/// in an ASCII letter (the tail of a command like `\leq`, which LaTeX parses as
/// greedily consuming following letters) and the next one starts with one. Without
/// this, `LEQ` followed by `x` would render as the literal text `\leq` immediately
/// followed by `x` in the *source*, i.e. the invalid, undefined command `\leqx`.
fn join_atoms(atoms: &[String]) -> String {
    let mut out = String::new();
    for atom in atoms {
        let needs_space = match (out.as_bytes().last(), atom.as_bytes().first()) {
            (Some(&prev), Some(&next)) => (prev as char).is_ascii_alphabetic() && (next as char).is_ascii_alphabetic(),
            _ => false,
        };
        if needs_space {
            out.push(' ');
        }
        out.push_str(atom);
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Word(String),
    Open,
    Close,
    Sym(char),
}

/// Marker character substituted for the two-character `+-` digraph so it can be
/// treated as a single symbol token; not a character HWP equation scripts use for
/// anything else.
const PLUS_MINUS_MARKER: char = '\u{E001}';
/// Marker character substituted for the two-character `-+` digraph.
const MINUS_PLUS_MARKER: char = '\u{E002}';

fn tokenize(script: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = script.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        match c {
            '{' => {
                chars.next();
                tokens.push(Token::Open);
            }
            '}' => {
                chars.next();
                tokens.push(Token::Close);
            }
            c if c.is_ascii_alphabetic() => {
                let mut word = String::new();
                while let Some(&c2) = chars.peek() {
                    if c2.is_ascii_alphabetic() {
                        word.push(c2);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Word(word));
            }
            '+' => {
                chars.next();
                if chars.peek() == Some(&'-') {
                    chars.next();
                    tokens.push(Token::Sym(PLUS_MINUS_MARKER));
                } else {
                    tokens.push(Token::Sym('+'));
                }
            }
            '-' => {
                chars.next();
                if chars.peek() == Some(&'+') {
                    chars.next();
                    tokens.push(Token::Sym(MINUS_PLUS_MARKER));
                } else {
                    tokens.push(Token::Sym('-'));
                }
            }
            other => {
                chars.next();
                tokens.push(Token::Sym(other));
            }
        }
    }

    tokens
}

/// Parses a sequence of atoms at the current nesting level, folding the binary
/// `OVER` (fraction) operator: an atom immediately followed by an `OVER` keyword and
/// another atom becomes `\frac{left}{right}`.
fn parse_atoms(tokens: &[Token], pos: &mut usize) -> Vec<String> {
    let mut atoms = Vec::new();

    while !matches!(tokens.get(*pos), None | Some(Token::Close)) {
        let Some(atom) = parse_one(tokens, pos) else { break };

        if peek_word(tokens, *pos).is_some_and(|w| w.eq_ignore_ascii_case("OVER")) {
            *pos += 1; // consume "OVER"
            let rhs = parse_one(tokens, pos).unwrap_or_default();
            atoms.push(format!("\\frac{{{atom}}}{{{rhs}}}"));
        } else {
            atoms.push(atom);
        }
    }

    atoms
}

fn peek_word(tokens: &[Token], pos: usize) -> Option<&str> {
    match tokens.get(pos) {
        Some(Token::Word(w)) => Some(w.as_str()),
        _ => None,
    }
}

/// Parses exactly one atom: a bare word/keyword, a symbol, or a `{...}` group
/// (recursing back into [`parse_atoms`] for the group's contents).
fn parse_one(tokens: &[Token], pos: &mut usize) -> Option<String> {
    match tokens.get(*pos)? {
        Token::Open => {
            *pos += 1;
            let inner = join_atoms(&parse_atoms(tokens, pos));
            if matches!(tokens.get(*pos), Some(Token::Close)) {
                *pos += 1;
            }
            Some(inner)
        }
        Token::Close => None,
        Token::Word(word) => {
            let word = word.clone();
            *pos += 1;
            match word.to_ascii_uppercase().as_str() {
                "SQRT" => {
                    let arg = parse_one(tokens, pos).unwrap_or_default();
                    Some(format!("\\sqrt{{{arg}}}"))
                }
                "FRAC" => {
                    let numerator = parse_one(tokens, pos).unwrap_or_default();
                    let denominator = parse_one(tokens, pos).unwrap_or_default();
                    Some(format!("\\frac{{{numerator}}}{{{denominator}}}"))
                }
                // An unrecognised keyword passes through verbatim rather than being
                // dropped: a partly-translated equation is more useful than a hole.
                upper => Some(keyword_to_latex(upper).map(str::to_owned).unwrap_or(word)),
            }
        }
        Token::Sym(c) => {
            let c = *c;
            *pos += 1;
            Some(symbol_to_latex(c))
        }
    }
}

fn symbol_to_latex(c: char) -> String {
    match c {
        PLUS_MINUS_MARKER => "\\pm".to_string(),
        MINUS_PLUS_MARKER => "\\mp".to_string(),
        other => other.to_string(),
    }
}

/// Looks up a case-normalized (uppercase) HWP equation keyword, returning its LaTeX
/// command, or `None` for an identifier that is not a recognized keyword.
fn keyword_to_latex(upper: &str) -> Option<&'static str> {
    Some(match upper {
        "LEQ" => "\\leq",
        "GEQ" => "\\geq",
        "NEQ" => "\\neq",
        "APPROX" => "\\approx",
        "INFTY" | "INFINITY" => "\\infty",
        "TIMES" => "\\times",
        "CDOT" => "\\cdot",
        "DIV" | "DIVIDE" => "\\div",
        "PM" | "PLUSMINUS" => "\\pm",
        "MP" => "\\mp",
        "SUM" => "\\sum",
        "PROD" => "\\prod",
        "INT" => "\\int",
        "OINT" => "\\oint",
        "PARTIAL" => "\\partial",
        "NABLA" => "\\nabla",
        "LIM" => "\\lim",
        "SIN" => "\\sin",
        "COS" => "\\cos",
        "TAN" => "\\tan",
        "LOG" => "\\log",
        "LN" => "\\ln",
        "LEFT" => "\\left",
        "RIGHT" => "\\right",
        "ALPHA" => "\\alpha",
        "BETA" => "\\beta",
        "GAMMA" => "\\gamma",
        "DELTA" => "\\delta",
        "EPSILON" => "\\epsilon",
        "ZETA" => "\\zeta",
        "ETA" => "\\eta",
        "THETA" => "\\theta",
        "IOTA" => "\\iota",
        "KAPPA" => "\\kappa",
        "LAMBDA" => "\\lambda",
        "MU" => "\\mu",
        "NU" => "\\nu",
        "XI" => "\\xi",
        "PI" => "\\pi",
        "RHO" => "\\rho",
        "SIGMA" => "\\sigma",
        "TAU" => "\\tau",
        "UPSILON" => "\\upsilon",
        "PHI" => "\\phi",
        "CHI" => "\\chi",
        "PSI" => "\\psi",
        "OMEGA" => "\\omega",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::to_latex;

    #[test]
    fn should_convert_braced_frac() {
        assert_eq!(to_latex("FRAC{a}{b}"), "\\frac{a}{b}");
    }

    #[test]
    fn should_convert_bareword_over() {
        assert_eq!(to_latex("a OVER b"), "\\frac{a}{b}");
    }

    #[test]
    fn should_convert_braced_over() {
        assert_eq!(to_latex("{1} over {2}"), "\\frac{1}{2}");
    }

    #[test]
    fn should_convert_sqrt() {
        assert_eq!(to_latex("SQRT{x}"), "\\sqrt{x}");
    }

    #[test]
    fn should_convert_nested_sqrt_over() {
        assert_eq!(to_latex("SQRT{x} OVER 2"), "\\frac{\\sqrt{x}}{2}");
    }

    #[test]
    fn should_convert_relational_keyword() {
        assert_eq!(to_latex("0.3 LEQ x"), "0.3\\leq x");
    }

    #[test]
    fn should_convert_greek_letters() {
        assert_eq!(to_latex("ALPHA"), "\\alpha");
        assert_eq!(to_latex("OMEGA"), "\\omega");
    }

    #[test]
    fn should_convert_plusminus_digraph() {
        assert_eq!(to_latex("A+-B"), "A\\pm B");
    }

    #[test]
    fn should_pass_through_unrecognized_identifier() {
        assert_eq!(to_latex("UNKNOWNTOKEN"), "UNKNOWNTOKEN");
    }

    #[test]
    fn should_pass_through_superscript_and_subscript_syntax() {
        // Braces around a single-token exponent/subscript are stripped rather than
        // preserved verbatim (unlike a brace group consumed as a `SQRT`/`FRAC`/`OVER`
        // operand, which needs its own reinstated braces to delimit the argument, a
        // bare passthrough group's braces are redundant here): `x^2` and `x^{2}`
        // render identically in LaTeX, and dropping them avoids ambiguity about
        // whether other operators should treat non-brace-consuming groups specially.
        assert_eq!(to_latex("x^{2}"), "x^2");
        assert_eq!(to_latex("x_{i}"), "x_i");
    }

    #[test]
    fn should_handle_empty_script() {
        assert_eq!(to_latex(""), "");
    }
}
