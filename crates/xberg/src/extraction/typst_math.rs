//! Typst math to LaTeX.
//!
//! `Formula.latex` holds LaTeX, so Typst math notation cannot go into it
//! verbatim: no renderer accepts `f_n = cases(a &"if" n = 0)`. The Typst
//! project's own parser (`typst-syntax`) reads the math, and the walk below
//! maps its tree to LaTeX.
//!
//! A construct with no LaTeX equivalent degrades rather than breaking the
//! output: an unknown symbol becomes upright text, an unknown function keeps
//! its name and arguments, and a layout-only argument (`size: #50%`) drops.
//! The result always parses.

use super::latex_shape::wrap_aligned_math;
use typst_syntax::{SyntaxKind, SyntaxNode, parse_math};

/// Convert one Typst math expression to LaTeX.
pub(crate) fn convert_typst_math_to_latex(source: &str) -> String {
    let root = parse_math(source);
    let latex = collapse_spaces(&render(&root));
    wrap_aligned_math(latex.trim())
}

/// Typst symbol names that differ from the LaTeX command, plus the ones whose
/// LaTeX spelling is worth pinning. A name not listed here falls back to
/// `\name` when it looks like a Greek letter or to upright text otherwise.
const SYMBOLS: &[(&str, &str)] = &[
    ("dot", "\\cdot"),
    ("dot.c", "\\cdot"),
    ("dot.op", "\\cdot"),
    ("times", "\\times"),
    ("div", "\\div"),
    ("plus.minus", "\\pm"),
    ("minus.plus", "\\mp"),
    ("star", "\\star"),
    ("ast", "\\ast"),
    ("circle", "\\circ"),
    ("infinity", "\\infty"),
    ("oo", "\\infty"),
    ("partial", "\\partial"),
    ("nabla", "\\nabla"),
    ("diff", "\\partial"),
    ("dots", "\\dots"),
    ("dots.h", "\\dots"),
    ("dots.h.c", "\\cdots"),
    ("dots.v", "\\vdots"),
    ("dots.down", "\\ddots"),
    ("arrow.r", "\\rightarrow"),
    ("arrow.l", "\\leftarrow"),
    ("arrow.t", "\\uparrow"),
    ("arrow.b", "\\downarrow"),
    ("arrow.r.double", "\\Rightarrow"),
    ("arrow.l.double", "\\Leftarrow"),
    ("arrow.l.r.double", "\\Leftrightarrow"),
    ("arrow.r.long", "\\longrightarrow"),
    ("in", "\\in"),
    ("in.not", "\\notin"),
    ("subset", "\\subset"),
    ("subset.eq", "\\subseteq"),
    ("supset", "\\supset"),
    ("supset.eq", "\\supseteq"),
    ("union", "\\cup"),
    ("sect", "\\cap"),
    ("union.big", "\\bigcup"),
    ("sect.big", "\\bigcap"),
    ("emptyset", "\\emptyset"),
    ("forall", "\\forall"),
    ("exists", "\\exists"),
    ("not", "\\neg"),
    ("and", "\\land"),
    ("or", "\\lor"),
    ("eq", "="),
    ("eq.not", "\\ne"),
    ("lt", "<"),
    ("lt.eq", "\\le"),
    ("gt", ">"),
    ("gt.eq", "\\ge"),
    ("approx", "\\approx"),
    ("equiv", "\\equiv"),
    ("prop", "\\propto"),
    ("tilde.op", "\\sim"),
    ("integral", "\\int"),
    ("integral.double", "\\iint"),
    ("integral.cont", "\\oint"),
    ("sum", "\\sum"),
    ("product", "\\prod"),
    ("limit", "\\lim"),
    ("floor.l", "\\lfloor"),
    ("floor.r", "\\rfloor"),
    ("ceil.l", "\\lceil"),
    ("ceil.r", "\\rceil"),
    ("angle.l", "\\langle"),
    ("angle.r", "\\rangle"),
    ("bar.v", "|"),
    ("bar.v.double", "\\|"),
    ("planck.reduce", "\\hbar"),
    ("ell", "\\ell"),
    ("aleph", "\\aleph"),
    ("degree", "^\\circ"),
    ("prime", "'"),
];

/// Greek letter names, which Typst and LaTeX spell the same way.
const GREEK: &[&str] = &[
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa", "lambda", "mu", "nu", "xi",
    "omicron", "pi", "rho", "sigma", "tau", "upsilon", "phi", "chi", "psi", "omega", "varepsilon", "vartheta", "varphi",
    "varrho", "varsigma", "Gamma", "Delta", "Theta", "Lambda", "Xi", "Pi", "Sigma", "Upsilon", "Phi", "Psi", "Omega",
];

/// Function names that wrap their single argument in a LaTeX command.
const WRAPPERS: &[(&str, &str)] = &[
    ("bold", "\\mathbf"),
    ("upright", "\\mathrm"),
    ("italic", "\\mathit"),
    ("sans", "\\mathsf"),
    ("mono", "\\mathtt"),
    ("cal", "\\mathcal"),
    ("frak", "\\mathfrak"),
    ("bb", "\\mathbb"),
    ("hat", "\\hat"),
    ("tilde", "\\tilde"),
    ("bar", "\\bar"),
    ("dot", "\\dot"),
    ("ddot", "\\ddot"),
    ("breve", "\\breve"),
    ("check", "\\check"),
    ("grave", "\\grave"),
    ("acute", "\\acute"),
    ("arrow", "\\vec"),
    ("vec", "\\vec"),
    ("overline", "\\overline"),
    ("underline", "\\underline"),
];

/// Operator names that LaTeX spells with a leading backslash.
const OPERATORS: &[&str] = &[
    "sin", "cos", "tan", "cot", "sec", "csc", "arcsin", "arccos", "arctan", "sinh", "cosh", "tanh", "log", "ln", "exp",
    "det", "dim", "ker", "deg", "gcd", "max", "min", "sup", "inf", "lim", "mod",
];

/// Render one node and its children.
fn render(node: &SyntaxNode) -> String {
    match node.kind() {
        SyntaxKind::Math => render_children(node),
        SyntaxKind::MathText | SyntaxKind::Text => escape_text(node.leaf_text()),
        SyntaxKind::MathIdent | SyntaxKind::Ident => symbol(node.leaf_text()),
        SyntaxKind::MathShorthand => shorthand(node.leaf_text()),
        SyntaxKind::MathAlignPoint => " & ".to_string(),
        SyntaxKind::Linebreak => " \\\\ ".to_string(),
        SyntaxKind::Str => text_command(node.leaf_text().trim_matches('"')),
        SyntaxKind::MathAttach => render_attach(node),
        SyntaxKind::MathFrac => render_frac(node),
        SyntaxKind::MathRoot => render_root(node),
        SyntaxKind::MathPrimes => node.leaf_text().to_string(),
        SyntaxKind::MathDelimited => render_children(node),
        SyntaxKind::MathCall | SyntaxKind::FuncCall => render_call(node),
        SyntaxKind::Space | SyntaxKind::Parbreak => " ".to_string(),
        SyntaxKind::LeftParen => "(".to_string(),
        SyntaxKind::RightParen => ")".to_string(),
        SyntaxKind::Comma => ", ".to_string(),
        SyntaxKind::Escape => node.leaf_text().trim_start_matches('\\').to_string(),
        // A layout argument, a comment, or a construct with no math meaning.
        SyntaxKind::Hash | SyntaxKind::LineComment | SyntaxKind::BlockComment | SyntaxKind::Error => String::new(),
        _ if node.children().len() > 0 => render_children(node),
        _ => escape_text(node.leaf_text()),
    }
}

fn render_children(node: &SyntaxNode) -> String {
    node.children().map(render).collect()
}

/// Render a node as a LaTeX group, dropping a delimiting pair of parentheses.
///
/// Typst writes `f_(n - 1)`, where the parentheses group the subscript rather
/// than appearing in it.
fn group(node: &SyntaxNode) -> String {
    let inner = group_inner(node);
    if inner.chars().count() == 1 { inner } else { format!("{{{inner}}}") }
}

/// Render a node as a braced LaTeX argument.
///
/// A single character may follow `_` or `^` bare, but never a command: `\frac`
/// with bare arguments reads as the undefined control sequence `\fracab`.
fn braced(node: &SyntaxNode) -> String {
    format!("{{{}}}", group_inner(node))
}

fn group_inner(node: &SyntaxNode) -> String {
    let rendered = collapse_spaces(&render(node)).trim().to_string();
    match rendered.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        Some(inner) => inner.trim().to_string(),
        None => rendered,
    }
}

fn render_attach(node: &SyntaxNode) -> String {
    let mut base = String::new();
    let mut out = String::new();
    let mut pending: Option<char> = None;
    for child in node.children() {
        match child.kind() {
            SyntaxKind::Underscore => pending = Some('_'),
            SyntaxKind::Hat => pending = Some('^'),
            SyntaxKind::Space => {}
            _ => match pending.take() {
                Some(marker) => {
                    out.push(marker);
                    out.push_str(&group(child));
                }
                None => base.push_str(&render(child)),
            },
        }
    }
    let base = base.trim();
    // A script with no base of its own still needs one, or two scripts in a row
    // read as a double subscript.
    if base.is_empty() && !out.is_empty() {
        return format!("{{}}{out}");
    }
    format!("{base}{out}")
}

fn render_frac(node: &SyntaxNode) -> String {
    let parts: Vec<&SyntaxNode> = node
        .children()
        .filter(|c| !matches!(c.kind(), SyntaxKind::Slash | SyntaxKind::Space))
        .collect();
    match parts.as_slice() {
        [num, den] => format!("\\frac{}{}", braced(num), braced(den)),
        _ => render_children(node),
    }
}

fn render_root(node: &SyntaxNode) -> String {
    let parts: Vec<&SyntaxNode> = node
        .children()
        .filter(|c| !matches!(c.kind(), SyntaxKind::Root | SyntaxKind::Space))
        .collect();
    match parts.as_slice() {
        [radicand] => format!("\\sqrt{}", braced(radicand)),
        [degree, radicand] => format!("\\sqrt[{}]{}", collapse_spaces(&render(degree)).trim(), braced(radicand)),
        _ => render_children(node),
    }
}

/// Split a call's arguments into rows and cells.
///
/// A comma separates cells and a semicolon separates rows, which is how Typst
/// writes `mat(1, 0; 0, 1)`. A named argument (`size: #50%`) is layout, not
/// math, so it drops.
fn call_rows(args: &SyntaxNode) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = vec![Vec::new()];
    let mut cell = String::new();
    for child in args.children() {
        match child.kind() {
            SyntaxKind::LeftParen | SyntaxKind::RightParen => {}
            SyntaxKind::Comma => {
                rows.last_mut().expect("one row exists").push(cell.trim().to_string());
                cell.clear();
            }
            SyntaxKind::Semicolon => {
                rows.last_mut().expect("one row exists").push(cell.trim().to_string());
                cell.clear();
                rows.push(Vec::new());
            }
            SyntaxKind::Named | SyntaxKind::Spread => {}
            _ => cell.push_str(&render(child)),
        }
    }
    let trailing = cell.trim().to_string();
    if !trailing.is_empty() {
        rows.last_mut().expect("one row exists").push(trailing);
    }
    for row in &mut rows {
        row.retain(|c| !c.is_empty());
    }
    rows.retain(|row| !row.is_empty());
    rows
}

fn environment(name: &str, rows: &[Vec<String>]) -> String {
    let body = rows
        .iter()
        .map(|row| row.join(" & "))
        .collect::<Vec<_>>()
        .join(" \\\\ ");
    format!("\\begin{{{name}}}{body}\\end{{{name}}}")
}

fn render_call(node: &SyntaxNode) -> String {
    let mut children = node.children();
    let Some(callee) = children.next() else {
        return render_children(node);
    };
    let name = callee.leaf_text().to_string();
    let Some(args) = children.find(|c| matches!(c.kind(), SyntaxKind::MathArgs | SyntaxKind::Args)) else {
        return symbol(&name);
    };
    let rows = call_rows(args);
    let flat: Vec<String> = rows.iter().flatten().cloned().collect();

    match name.as_str() {
        "frac" if flat.len() == 2 => format!("\\frac{{{}}}{{{}}}", flat[0], flat[1]),
        "sqrt" if flat.len() == 1 => format!("\\sqrt{{{}}}", flat[0]),
        "root" if flat.len() == 2 => format!("\\sqrt[{}]{{{}}}", flat[0], flat[1]),
        // A comma separates the branches of `cases`, so each argument is a row
        // rather than a cell.
        "cases" => environment("cases", &flat.iter().map(|c| vec![c.clone()]).collect::<Vec<_>>()),
        "mat" => environment("pmatrix", &rows),
        "vec" if !flat.is_empty() => environment("pmatrix", &flat.iter().map(|c| vec![c.clone()]).collect::<Vec<_>>()),
        "abs" if flat.len() == 1 => format!("\\left|{}\\right|", flat[0]),
        "norm" if flat.len() == 1 => format!("\\left\\|{}\\right\\|", flat[0]),
        // `lr` asks Typst to size delimiters that the content already carries.
        "lr" => flat.join(" "),
        "underbrace" if flat.len() == 2 => format!("\\underbrace{{{}}}_{{{}}}", flat[0], flat[1]),
        "underbrace" if flat.len() == 1 => format!("\\underbrace{{{}}}", flat[0]),
        "overbrace" if flat.len() == 2 => format!("\\overbrace{{{}}}^{{{}}}", flat[0], flat[1]),
        "overbrace" if flat.len() == 1 => format!("\\overbrace{{{}}}", flat[0]),
        "text" if flat.len() == 1 => text_command(&flat[0]),
        "op" if flat.len() == 1 => format!("\\operatorname{{{}}}", flat[0]),
        _ => match WRAPPERS.iter().find(|(typst, _)| *typst == name) {
            Some((_, command)) if flat.len() == 1 => format!("{command}{{{}}}", flat[0]),
            // An unknown function keeps its name and arguments, which parses
            // and shows the reader what the source held.
            _ => format!("\\mathrm{{{}}}({})", escape_text(&name), flat.join(", ")),
        },
    }
}

/// Map a Typst symbol name to LaTeX.
fn symbol(name: &str) -> String {
    if let Some((_, latex)) = SYMBOLS.iter().find(|(typst, _)| *typst == name) {
        return format!("{latex} ");
    }
    if GREEK.contains(&name) {
        return format!("\\{name} ");
    }
    if OPERATORS.contains(&name) {
        return format!("\\{name} ");
    }
    // A dotted name whose full form is unknown falls back to its base:
    // `arrow.r.long.bar` still reads as an arrow.
    if let Some((base, _)) = name.split_once('.')
        && (SYMBOLS.iter().any(|(typst, _)| *typst == base) || GREEK.contains(&base))
    {
        return symbol(base);
    }
    if name.chars().count() == 1 {
        return escape_text(name);
    }
    format!("\\mathrm{{{}}}", escape_text(name))
}

/// Map a Typst shorthand to its LaTeX command.
fn shorthand(text: &str) -> String {
    let latex = match text {
        "->" => "\\to",
        "<-" => "\\leftarrow",
        "<->" => "\\leftrightarrow",
        "=>" => "\\Rightarrow",
        "<=>" => "\\Leftrightarrow",
        "|->" => "\\mapsto",
        ">=" => "\\ge",
        "<=" => "\\le",
        "!=" => "\\ne",
        "==" => "\\equiv",
        "..." => "\\dots",
        "[|" => "\\llbracket",
        "|]" => "\\rrbracket",
        "-" => "-",
        _ => return escape_text(text),
    };
    format!("{latex} ")
}

/// Wrap prose in `\text{}`, escaping what LaTeX would otherwise read as markup.
fn text_command(text: &str) -> String {
    let mut inner = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => inner.push_str("\\textbackslash{}"),
            '{' => inner.push_str("\\{"),
            '}' => inner.push_str("\\}"),
            '#' => inner.push_str("\\#"),
            '%' => inner.push_str("\\%"),
            '$' => inner.push_str("\\$"),
            '&' => inner.push_str("\\&"),
            '_' => inner.push_str("\\_"),
            '^' => inner.push_str("\\textasciicircum{}"),
            '~' => inner.push_str("\\textasciitilde{}"),
            _ => inner.push(ch),
        }
    }
    format!("\\text{{{inner}}}")
}

/// Escape the characters that would change the structure of the LaTeX.
fn escape_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\backslash "),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '#' => out.push_str("\\#"),
            '%' => out.push_str("\\%"),
            '$' => out.push_str("\\$"),
            '&' => out.push_str("\\&"),
            _ => out.push(ch),
        }
    }
    out
}



/// Collapse runs of whitespace, which the walk emits freely around commands.
fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last_space = false;
    for ch in text.chars() {
        let is_space = ch.is_whitespace();
        if is_space {
            if !last_space {
                out.push(' ');
            }
        } else {
            out.push(ch);
        }
        last_space = is_space;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attach_becomes_sub_and_superscript() {
        assert_eq!(convert_typst_math_to_latex("x^2"), "x^2");
        assert_eq!(convert_typst_math_to_latex("f_n"), "f_n");
        assert_eq!(convert_typst_math_to_latex("f_(n - 1)"), "f_{n - 1}");
        assert_eq!(convert_typst_math_to_latex("x_i^2"), "x_i^2");
    }

    #[test]
    fn test_symbols_become_commands() {
        assert_eq!(convert_typst_math_to_latex("alpha + beta"), "\\alpha + \\beta");
        assert_eq!(convert_typst_math_to_latex("nabla dot v"), "\\nabla \\cdot v");
        assert_eq!(convert_typst_math_to_latex("a times b"), "a \\times b");
    }

    #[test]
    fn test_functions_become_commands() {
        assert_eq!(convert_typst_math_to_latex("frac(a, b)"), "\\frac{a}{b}");
        assert_eq!(convert_typst_math_to_latex("sqrt(x + 1)"), "\\sqrt{x + 1}");
        assert_eq!(convert_typst_math_to_latex("root(3, x)"), "\\sqrt[3]{x}");
        assert_eq!(convert_typst_math_to_latex("bold(D)"), "\\mathbf{D}");
        assert_eq!(convert_typst_math_to_latex("hat(y)"), "\\hat{y}");
    }

    #[test]
    fn test_matrix_and_vector_become_environments() {
        assert_eq!(
            convert_typst_math_to_latex("mat(1, 0; 0, 1)"),
            "\\begin{pmatrix}1 & 0 \\\\ 0 & 1\\end{pmatrix}"
        );
        assert_eq!(
            convert_typst_math_to_latex("vec(a, b)"),
            "\\begin{pmatrix}a \\\\ b\\end{pmatrix}"
        );
    }

    #[test]
    fn test_cases_keeps_its_branches() {
        let latex = convert_typst_math_to_latex("cases(a &\"if\" n = 0, b &\"else\")");
        assert!(latex.starts_with("\\begin{cases}"), "got: {latex}");
        assert!(latex.contains("\\text{if}"), "got: {latex}");
        assert!(latex.contains("\\\\"), "branches are separated: {latex}");
    }

    #[test]
    fn test_alignment_gets_an_environment() {
        let latex = convert_typst_math_to_latex("a &= b \\\nc &= d");
        assert!(latex.starts_with("\\begin{aligned}"), "got: {latex}");
        assert!(latex.ends_with("\\end{aligned}"), "got: {latex}");
    }

    /// A layout argument has no math meaning, so it drops rather than reaching
    /// the LaTeX, where `#50%` would not parse.
    #[test]
    fn test_layout_argument_drops() {
        let latex = convert_typst_math_to_latex("lr([sum_(k = 0)^n e^(k^2)], size: #50%)");
        assert!(!latex.contains('#'), "got: {latex}");
        assert!(!latex.contains('%'), "got: {latex}");
        assert!(latex.contains("\\sum"), "the content survives: {latex}");
    }

    /// An unknown function keeps its name and arguments so the output still
    /// parses and still shows what the source held.
    #[test]
    fn test_unknown_function_degrades_to_upright_text() {
        let latex = convert_typst_math_to_latex("wibble(x, y)");
        assert_eq!(latex, "\\mathrm{wibble}(x, y)");
    }

    #[test]
    fn test_unknown_symbol_degrades_to_upright_text() {
        assert_eq!(convert_typst_math_to_latex("wibblesymbol"), "\\mathrm{wibblesymbol}");
    }

    #[test]
    fn test_underbrace_takes_its_annotation() {
        assert_eq!(
            convert_typst_math_to_latex("underbrace(x + y, |A|)"),
            "\\underbrace{x + y}_{|A|}"
        );
    }

    #[test]
    fn test_quoted_text_becomes_a_text_command() {
        assert_eq!(convert_typst_math_to_latex("\"total\""), "\\text{total}");
    }

    #[test]
    fn test_shorthands_become_commands() {
        assert_eq!(convert_typst_math_to_latex("a -> b"), "a \\to b");
        assert_eq!(convert_typst_math_to_latex("a <= b"), "a \\le b");
    }

    /// Whatever the source holds, the output carries no unescaped character
    /// that would end the formula or start a group.
    #[test]
    fn test_output_escapes_structural_characters() {
        let latex = convert_typst_math_to_latex("\"a $ b # c % d\"");
        assert!(latex.contains("\\$") && latex.contains("\\#") && latex.contains("\\%"), "got: {latex}");
    }
}
