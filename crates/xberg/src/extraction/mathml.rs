//! Presentation MathML to LaTeX converter.
//!
//! Converts `<math>` (MathML) subtrees found in ODT/ODP embedded formula objects
//! and EPUB XHTML content to LaTeX notation. Modeled on the OMML converter at
//! `docx::math`: the subtree is collected into an `MmlNode` tree, then recursively
//! rendered to LaTeX. Unknown/unhandled elements degrade to their text content
//! instead of failing the whole document.
//!
//! Unlike OMML (which streams off a `quick_xml::Reader` while parsing DOCX part
//! XML), callers here already hold a parsed `roxmltree::Document` — ODT parses an
//! embedded object's `content.xml` on its own, and EPUB walks XHTML with
//! `roxmltree` already. So the converter operates directly on a `roxmltree::Node`,
//! with a `&str`-in convenience wrapper for callers that only have raw XML text.

use crate::extraction::math_symbols::render_run_text;
use crate::extractors::security::{SecurityBudget, SecurityError};
use roxmltree::Node;

/// Names of MathML elements that hold no rendered content and whose text (an
/// alternate-encoding annotation, e.g. `StarMath` or content-MathML) must never
/// leak into the LaTeX output.
const ANNOTATION_ELEMENTS: &[&str] = &["annotation", "annotation-xml"];

/// Names of MathML elements that are pure grouping/styling wrappers: their
/// children are rendered in sequence with no LaTeX markup of their own.
const TRANSPARENT_ELEMENTS: &[&str] = &["math", "mrow", "mstyle", "mpadded", "merror"];

#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
enum MmlNode {
    /// Plain text from `mi`/`mn`/`mo`/`ms`.
    Run(String),
    /// Literal text from `mtext`: rendered as `\text{...}`.
    Text(String),
    /// A single blank space from `mspace`.
    Space,
    /// Fraction: `\frac{num}{den}`.
    Frac { num: Box<MmlNode>, den: Box<MmlNode> },
    /// Superscript: `base^{sup}`.
    Sup { base: Box<MmlNode>, sup: Box<MmlNode> },
    /// Subscript: `base_{sub}`.
    Sub { base: Box<MmlNode>, sub: Box<MmlNode> },
    /// Sub-superscript: `base_{sub}^{sup}`.
    SubSup {
        base: Box<MmlNode>,
        sub: Box<MmlNode>,
        sup: Box<MmlNode>,
    },
    /// Square root: `\sqrt{body}`.
    Sqrt { body: Box<MmlNode> },
    /// N-th root: `\sqrt[index]{body}`.
    Root { body: Box<MmlNode>, index: Box<MmlNode> },
    /// Fenced group: `\left<open> a, b, ...\right<close>`.
    Fenced {
        open: String,
        close: String,
        sep: String,
        elements: Vec<MmlNode>,
    },
    /// Underscript: `\underset{under}{base}`.
    Under { base: Box<MmlNode>, under: Box<MmlNode> },
    /// Overscript: `\overset{over}{base}`.
    Over { base: Box<MmlNode>, over: Box<MmlNode> },
    /// Under+overscript: `\overset{over}{\underset{under}{base}}`.
    UnderOver {
        base: Box<MmlNode>,
        under: Box<MmlNode>,
        over: Box<MmlNode>,
    },
    /// Phantom (invisible but space-occupying): `\phantom{body}`.
    Phantom { body: Box<MmlNode> },
    /// Table: `\begin{matrix}...\end{matrix}`.
    Table { rows: Vec<Vec<MmlNode>> },
    /// Grouping container (`math`, `mrow`, `semantics` presentation branch,
    /// unknown elements) — renders its children in sequence.
    Group { children: Vec<MmlNode> },
}

/// Convert a MathML XML fragment (a full document whose root is `<math>`, or
/// `<math>` nested anywhere in the fragment) to LaTeX.
///
/// Used by callers (e.g. ODT's embedded-object formula extraction) that only
/// have the raw XML text of a formula and have not already parsed it.
pub(crate) fn convert_mathml_str_to_latex(xml: &str, budget: &mut SecurityBudget) -> Result<String, SecurityError> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Ok(String::new());
    };

    let root = doc.root_element();
    let math_node = if root.tag_name().name().eq_ignore_ascii_case("math") {
        root
    } else {
        match root
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name().eq_ignore_ascii_case("math"))
        {
            Some(node) => node,
            None => root,
        }
    };

    convert_mathml_node_to_latex(math_node, budget)
}

/// Convert an already-parsed MathML `<math>` (or presentation) node to LaTeX.
///
/// Used by callers (e.g. the EPUB XHTML walker) that already hold a
/// `roxmltree::Node` positioned at the `<math>` element.
pub(crate) fn convert_mathml_node_to_latex(node: Node, budget: &mut SecurityBudget) -> Result<String, SecurityError> {
    let collected = collect_node(node, budget)?;
    let mut out = String::new();
    render_node(&collected, &mut out);
    Ok(out)
}

/// Collect an element's children into a sequence of `MmlNode`s, dispatching each
/// child element through [`collect_node`] and each direct text node into a
/// [`MmlNode::Run`]. Whitespace-only text nodes are dropped.
fn collect_children(parent: Node, budget: &mut SecurityBudget) -> Result<Vec<MmlNode>, SecurityError> {
    let mut nodes = Vec::new();
    for child in parent.children() {
        budget.step()?;
        if child.is_element() {
            nodes.push(collect_node(child, budget)?);
        } else if child.is_text() {
            let text = child.text().unwrap_or("");
            if !text.trim().is_empty() {
                budget.check_entity(text)?;
                budget.account_text(text.len())?;
                nodes.push(MmlNode::Run(text.to_string()));
            }
        }
    }
    Ok(nodes)
}

/// Collect the Nth element child of `parent` (skipping non-element nodes) into a
/// single `MmlNode`, or an empty `Group` if fewer than `index + 1` element
/// children exist.
fn collect_nth_child(parent: Node, index: usize, budget: &mut SecurityBudget) -> Result<MmlNode, SecurityError> {
    match parent.children().filter(|c| c.is_element()).nth(index) {
        Some(child) => collect_node(child, budget),
        None => Ok(MmlNode::Group { children: Vec::new() }),
    }
}

/// Collect a single MathML element into an `MmlNode`, dispatching on tag name.
fn collect_node(node: Node, budget: &mut SecurityBudget) -> Result<MmlNode, SecurityError> {
    budget.step()?;
    budget.enter()?;
    let result = collect_node_inner(node, budget);
    budget.leave();
    result
}

fn collect_node_inner(node: Node, budget: &mut SecurityBudget) -> Result<MmlNode, SecurityError> {
    let tag = node.tag_name().name();

    if ANNOTATION_ELEMENTS.iter().any(|&s| s.eq_ignore_ascii_case(tag)) {
        return Ok(MmlNode::Group { children: Vec::new() });
    }

    match tag.to_ascii_lowercase().as_str() {
        "mi" | "mn" | "ms" | "mo" => Ok(MmlNode::Run(collect_text(node, budget)?)),
        "mtext" => Ok(MmlNode::Text(collect_text(node, budget)?)),
        "mspace" => Ok(MmlNode::Space),
        "semantics" => {
            let children = node
                .children()
                .filter(|c| {
                    c.is_element()
                        && !ANNOTATION_ELEMENTS
                            .iter()
                            .any(|&s| s.eq_ignore_ascii_case(c.tag_name().name()))
                })
                .map(|c| collect_node(c, budget))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(MmlNode::Group { children })
        }
        t if TRANSPARENT_ELEMENTS.contains(&t) => Ok(MmlNode::Group {
            children: collect_children(node, budget)?,
        }),
        "mfrac" => Ok(MmlNode::Frac {
            num: Box::new(collect_nth_child(node, 0, budget)?),
            den: Box::new(collect_nth_child(node, 1, budget)?),
        }),
        "msup" => Ok(MmlNode::Sup {
            base: Box::new(collect_nth_child(node, 0, budget)?),
            sup: Box::new(collect_nth_child(node, 1, budget)?),
        }),
        "msub" => Ok(MmlNode::Sub {
            base: Box::new(collect_nth_child(node, 0, budget)?),
            sub: Box::new(collect_nth_child(node, 1, budget)?),
        }),
        "msubsup" => Ok(MmlNode::SubSup {
            base: Box::new(collect_nth_child(node, 0, budget)?),
            sub: Box::new(collect_nth_child(node, 1, budget)?),
            sup: Box::new(collect_nth_child(node, 2, budget)?),
        }),
        "msqrt" => Ok(MmlNode::Sqrt {
            body: Box::new(MmlNode::Group {
                children: collect_children(node, budget)?,
            }),
        }),
        "mroot" => Ok(MmlNode::Root {
            body: Box::new(collect_nth_child(node, 0, budget)?),
            index: Box::new(collect_nth_child(node, 1, budget)?),
        }),
        "mfenced" => collect_fenced(node, budget),
        "munder" => Ok(MmlNode::Under {
            base: Box::new(collect_nth_child(node, 0, budget)?),
            under: Box::new(collect_nth_child(node, 1, budget)?),
        }),
        "mover" => Ok(MmlNode::Over {
            base: Box::new(collect_nth_child(node, 0, budget)?),
            over: Box::new(collect_nth_child(node, 1, budget)?),
        }),
        "munderover" => Ok(MmlNode::UnderOver {
            base: Box::new(collect_nth_child(node, 0, budget)?),
            under: Box::new(collect_nth_child(node, 1, budget)?),
            over: Box::new(collect_nth_child(node, 2, budget)?),
        }),
        "mphantom" => Ok(MmlNode::Phantom {
            body: Box::new(MmlNode::Group {
                children: collect_children(node, budget)?,
            }),
        }),
        "mtable" => collect_table(node, budget),
        _ => Ok(MmlNode::Group {
            children: collect_children(node, budget)?,
        }),
    }
}

/// Collect the direct text content of a leaf element (`mi`/`mn`/`mo`/`ms`/`mtext`).
fn collect_text(node: Node, budget: &mut SecurityBudget) -> Result<String, SecurityError> {
    let mut text = String::new();
    // Only real text nodes — `Node::text()` also returns content for comment
    // nodes, and MathML fixtures commonly annotate entities with a comment
    // (e.g. `<mo>&#x222B;<!-- ∫ --></mo>`) that must not be double-counted. ~keep
    for child in node.children().filter(|c| c.is_text()) {
        if let Some(t) = child.text() {
            budget.check_entity(t)?;
            budget.account_text(t.len())?;
            text.push_str(t);
        }
    }
    Ok(text)
}

/// Collect an `mfenced` element: `open`/`close`/`separators` attributes plus
/// one element per fenced argument.
fn collect_fenced(node: Node, budget: &mut SecurityBudget) -> Result<MmlNode, SecurityError> {
    let open = node.attribute("open").unwrap_or("(").to_string();
    let close = node.attribute("close").unwrap_or(")").to_string();
    let sep = node
        .attribute("separators")
        .and_then(|s| s.chars().next())
        .unwrap_or(',')
        .to_string();

    let elements = node
        .children()
        .filter(|c| c.is_element())
        .map(|c| collect_node(c, budget))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(MmlNode::Fenced {
        open,
        close,
        sep,
        elements,
    })
}

/// Collect an `mtable` element into rows of cells (`mtr` > `mtd`).
fn collect_table(node: Node, budget: &mut SecurityBudget) -> Result<MmlNode, SecurityError> {
    let mut rows = Vec::new();
    for row in node
        .children()
        .filter(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case("mtr"))
    {
        budget.step()?;
        let cells = row
            .children()
            .filter(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case("mtd"))
            .map(|c| {
                Ok(MmlNode::Group {
                    children: collect_children(c, budget)?,
                })
            })
            .collect::<Result<Vec<_>, SecurityError>>()?;
        rows.push(cells);
    }
    Ok(MmlNode::Table { rows })
}

/// Render a slice of `MmlNode`s to LaTeX, concatenated with no separators.
fn render_nodes(nodes: &[MmlNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        render_node(node, &mut out);
    }
    out
}

/// Render a single `MmlNode` to LaTeX, appending to `out`.
fn render_node(node: &MmlNode, out: &mut String) {
    match node {
        MmlNode::Run(text) => render_run_text(text, out),
        MmlNode::Text(text) => render_text_content(text, out),
        MmlNode::Space => out.push(' '),
        MmlNode::Frac { num, den } => {
            out.push_str("\\frac{");
            render_node(num, out);
            out.push_str("}{");
            render_node(den, out);
            out.push('}');
        }
        MmlNode::Sup { base, sup } => {
            render_arg(base, out);
            out.push_str("^{");
            render_node(sup, out);
            out.push('}');
        }
        MmlNode::Sub { base, sub } => {
            render_arg(base, out);
            out.push_str("_{");
            render_node(sub, out);
            out.push('}');
        }
        MmlNode::SubSup { base, sub, sup } => {
            render_arg(base, out);
            out.push_str("_{");
            render_node(sub, out);
            out.push_str("}^{");
            render_node(sup, out);
            out.push('}');
        }
        MmlNode::Sqrt { body } => {
            out.push_str("\\sqrt{");
            render_node(body, out);
            out.push('}');
        }
        MmlNode::Root { body, index } => {
            out.push_str("\\sqrt[");
            render_node(index, out);
            out.push_str("]{");
            render_node(body, out);
            out.push('}');
        }
        MmlNode::Fenced {
            open,
            close,
            sep,
            elements,
        } => {
            // Authors use `mfenced` as plain grouping with operators as direct
            // children; inserting the spec-default comma separators there turns
            // `(1 - x)` into `(1,-,x)`. Suppress separators when any child is
            // itself an infix operator.
            let sep = if elements.iter().any(is_operator_child) { "" } else { sep.as_str() };
            let (left, right) = (fence_chr_to_latex(open), fence_chr_to_latex(close));
            match (left, right) {
                (Some(left), Some(right)) => {
                    out.push_str("\\left");
                    out.push_str(left);
                    for (i, elem) in elements.iter().enumerate() {
                        if i > 0 {
                            out.push_str(sep);
                        }
                        render_node(elem, out);
                    }
                    out.push_str("\\right");
                    out.push_str(right);
                }
                // A fence char LaTeX cannot use after `\left`: emit the fences
                // as plain glyphs instead of producing an unparseable string.
                _ => {
                    render_run_text(open, out);
                    for (i, elem) in elements.iter().enumerate() {
                        if i > 0 {
                            out.push_str(sep);
                        }
                        render_node(elem, out);
                    }
                    render_run_text(close, out);
                }
            }
        }
        MmlNode::Under { base, under } => {
            if let Some(cmd) = under_script_command(under) {
                out.push_str(cmd);
                out.push('{');
                render_node(base, out);
                out.push('}');
            } else {
                out.push_str("\\underset{");
                render_node(under, out);
                out.push_str("}{");
                render_node(base, out);
                out.push('}');
            }
        }
        MmlNode::Over { base, over } => {
            if let Some(cmd) = over_script_command(over, base) {
                out.push_str(cmd);
                out.push('{');
                render_node(base, out);
                out.push('}');
            } else {
                out.push_str("\\overset{");
                render_node(over, out);
                out.push_str("}{");
                render_node(base, out);
                out.push('}');
            }
        }
        MmlNode::UnderOver { base, under, over } => {
            out.push_str("\\overset{");
            render_node(over, out);
            out.push_str("}{\\underset{");
            render_node(under, out);
            out.push_str("}{");
            render_node(base, out);
            out.push_str("}}");
        }
        MmlNode::Phantom { body } => {
            out.push_str("\\phantom{");
            render_node(body, out);
            out.push('}');
        }
        MmlNode::Table { rows } => {
            out.push_str("\\begin{matrix}");
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(" \\\\ ");
                }
                for (j, cell) in row.iter().enumerate() {
                    if j > 0 {
                        out.push_str(" & ");
                    }
                    render_node(cell, out);
                }
            }
            out.push_str("\\end{matrix}");
        }
        MmlNode::Group { children } => out.push_str(&render_nodes(children)),
    }
}

/// True when `s` is exactly one balanced brace group (`{...}`): the opening
/// brace's closer is the final character. `{a}^{b}` starts with `{` and ends
/// with `}` but is two atoms — treating it as pre-braced produces double
/// scripts when another script attaches.
fn is_single_brace_group(s: &str) -> bool {
    if !s.starts_with('{') || !s.ends_with('}') {
        return false;
    }
    let mut depth = 0usize;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i == s.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

/// Render `mtext` content. Plain text goes inside `\text{...}` with text-mode
/// escaping; characters that map to math commands (Greek letters, operators)
/// are emitted *outside* the `\text` group, because commands like `\Delta` are
/// math-mode-only and fail inside `\text{}`.
fn render_text_content(text: &str, out: &mut String) {
    let mut in_text = false;
    for ch in text.chars() {
        if let Some(latex) = crate::extraction::math_symbols::unicode_to_latex(ch) {
            if in_text {
                out.push('}');
                in_text = false;
            }
            out.push_str(latex);
            continue;
        }
        if !in_text {
            out.push_str("\\text{");
            in_text = true;
        }
        match ch {
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '_' => out.push_str("\\_"),
            '\\' => out.push_str("\\textbackslash "),
            '^' => out.push_str("\\textasciicircum "),
            '~' => out.push_str("\\textasciitilde "),
            _ => out.push(ch),
        }
    }
    if in_text {
        out.push('}');
    }
}

/// The raw script text of an accent-like script node (`mo`/`mi` leaf, possibly
/// inside grouping), or `None` when the script is real content.
fn script_leaf_text(node: &MmlNode) -> Option<&str> {
    match node {
        MmlNode::Run(text) => Some(text.trim()),
        MmlNode::Group { children } if children.len() == 1 => script_leaf_text(&children[0]),
        _ => None,
    }
}

/// True when the base renders to a single glyph (possibly one LaTeX command),
/// used to pick `\bar`/`\vec` over `\overline`/`\overrightarrow`.
fn base_is_single_glyph(base: &MmlNode) -> bool {
    let mut rendered = String::new();
    render_node(base, &mut rendered);
    let t = rendered.trim();
    t.chars().count() == 1 || (t.starts_with('\\') && t[1..].chars().all(|c| c.is_ascii_alphabetic()))
}

/// Map an `mover` script char to a LaTeX accent command. MathML sources write
/// accents as literal combining/spacing characters (`<mover><mi>x</mi>
/// <mo>^</mo></mover>`); `\overset{^}{x}` is not valid LaTeX (bare `^` needs a
/// group), so these must become accent macros.
fn over_script_command(over: &MmlNode, base: &MmlNode) -> Option<&'static str> {
    match script_leaf_text(over)? {
        "^" | "\u{02C6}" | "\u{0302}" => Some("\\hat"),
        "~" | "\u{02DC}" | "\u{0303}" | "\u{223C}" => Some("\\tilde"),
        "\u{02D9}" | "\u{0307}" => Some("\\dot"),
        "\u{00A8}" | "\u{0308}" => Some("\\ddot"),
        "\u{00AF}" | "\u{203E}" | "\u{0304}" | "\u{0305}" => {
            Some(if base_is_single_glyph(base) { "\\bar" } else { "\\overline" })
        }
        "\u{2192}" | "\u{20D7}" => Some(if base_is_single_glyph(base) {
            "\\vec"
        } else {
            "\\overrightarrow"
        }),
        "\u{02C7}" | "\u{030C}" => Some("\\check"),
        "\u{02D8}" | "\u{0306}" => Some("\\breve"),
        "\u{00B4}" | "\u{0301}" => Some("\\acute"),
        "`" | "\u{0300}" => Some("\\grave"),
        "\u{02DA}" | "\u{030A}" => Some("\\mathring"),
        "\u{23DE}" => Some("\\overbrace"),
        _ => None,
    }
}

/// Map an `munder` script char to a LaTeX command, like [`over_script_command`].
fn under_script_command(under: &MmlNode) -> Option<&'static str> {
    match script_leaf_text(under)? {
        "_" | "\u{0332}" | "\u{02CD}" | "\u{00AF}" | "\u{203E}" => Some("\\underline"),
        "\u{23DF}" => Some("\\underbrace"),
        _ => None,
    }
}

/// Render an argument (sup/sub base), wrapping in braces unless it is a single
/// atom (one character, one LaTeX command, or one brace group).
///
/// A compound base that already carries a script (`\lambda _{1}^{'}`) MUST be
/// wrapped, or attaching the outer script produces a double superscript. An
/// empty base (script-only markup like tensor `{}_{,\nu}`) renders as `{}` so
/// the script cannot fuse onto the preceding atom.
fn render_arg(node: &MmlNode, out: &mut String) {
    let mut rendered = String::new();
    render_node(node, &mut rendered);
    let trimmed = rendered.trim();
    if trimmed.is_empty() {
        out.push_str("{}");
        return;
    }
    let single_char = trimmed.chars().count() == 1;
    let single_command = trimmed.starts_with('\\')
        && trimmed.len() > 1
        && trimmed[1..].chars().all(|c| c.is_ascii_alphabetic());
    if single_char || single_command || is_single_brace_group(trimmed) {
        out.push_str(&rendered);
    } else {
        out.push('{');
        out.push_str(trimmed);
        out.push('}');
    }
}

/// True when a fenced child renders to a bare infix operator, meaning the
/// `mfenced` is grouping an expression, not listing arguments.
fn is_operator_child(node: &MmlNode) -> bool {
    let MmlNode::Run(text) = node else { return false };
    matches!(
        text.trim(),
        "+" | "-"
            | "\u{2212}"
            | "="
            | "\u{00B1}"
            | "\u{00D7}"
            | "\u{22C5}"
            | "/"
            | "<"
            | ">"
            | "\u{2264}"
            | "\u{2265}"
    )
}

/// Map an `mfenced` open/close character to a LaTeX delimiter valid after
/// `\left`/`\right`, or `None` for characters LaTeX cannot use there.
/// Word-form commands carry a trailing space so following content never glues
/// onto the control word (`\langle A`, not `\langleA`).
fn fence_chr_to_latex(chr: &str) -> Option<&'static str> {
    match chr {
        "(" => Some("("),
        ")" => Some(")"),
        "[" => Some("["),
        "]" => Some("]"),
        "{" => Some("\\{"),
        "}" => Some("\\}"),
        "|" | "\u{2223}" => Some("|"),
        "\u{2016}" | "\u{2225}" => Some("\\|"),
        "\u{2329}" | "\u{27E8}" => Some("\\langle "),
        "\u{232A}" | "\u{27E9}" => Some("\\rangle "),
        "\u{230A}" => Some("\\lfloor "),
        "\u{230B}" => Some("\\rfloor "),
        "\u{2308}" => Some("\\lceil "),
        "\u{2309}" => Some("\\rceil "),
        "/" => Some("/"),
        "\\" => Some("\\backslash "),
        "" => Some("."),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse a MathML XML fragment and return rendered LaTeX.
    fn mathml_to_latex(inner: &str) -> String {
        let xml = format!(r#"<math xmlns="http://www.w3.org/1998/Math/MathML">{}</math>"#, inner);
        let mut budget = SecurityBudget::with_defaults();
        convert_mathml_str_to_latex(&xml, &mut budget).expect("conversion ok")
    }

    #[test]
    fn test_mi_plain_text() {
        assert_eq!(mathml_to_latex("<mi>x</mi>"), "x");
    }

    #[test]
    fn test_mn_number() {
        assert_eq!(mathml_to_latex("<mn>42</mn>"), "42");
    }

    #[test]
    fn test_mo_unicode_operator() {
        assert_eq!(mathml_to_latex("<mo>\u{00D7}</mo>"), "\\times ");
    }

    #[test]
    fn test_numeric_char_ref_with_trailing_comment_is_not_duplicated() {
        // Real-world MathML (e.g. EPUB accessibility test suites) commonly
        // annotates a numeric character reference with a same-content XML
        // comment: `<mo>&#x222B;<!-- ∫ --></mo>`. The comment must not be
        // rendered a second time alongside the decoded entity. ~keep
        assert_eq!(mathml_to_latex("<mo>&#x222B;<!-- \u{222B} --></mo>"), "\\int ");
        assert_eq!(
            mathml_to_latex("<mi mathvariant=\"normal\">&#x221E;<!-- \u{221E} --></mi>"),
            "\\infty "
        );
    }

    #[test]
    fn test_mtext_wraps_in_text_command() {
        assert_eq!(mathml_to_latex("<mtext>hello world</mtext>"), "\\text{hello world}");
    }

    #[test]
    fn test_ms_string_literal() {
        assert_eq!(mathml_to_latex("<ms>abc</ms>"), "abc");
    }

    #[test]
    fn test_mrow_concatenates_children() {
        assert_eq!(mathml_to_latex("<mrow><mi>x</mi><mo>+</mo><mi>y</mi></mrow>"), "x+y");
    }

    #[test]
    fn test_mfrac() {
        assert_eq!(mathml_to_latex("<mfrac><mn>1</mn><mn>2</mn></mfrac>"), "\\frac{1}{2}");
    }

    #[test]
    fn test_msup() {
        assert_eq!(mathml_to_latex("<msup><mi>x</mi><mn>2</mn></msup>"), "x^{2}");
    }

    #[test]
    fn test_msub() {
        assert_eq!(mathml_to_latex("<msub><mi>a</mi><mi>n</mi></msub>"), "a_{n}");
    }

    #[test]
    fn test_msubsup() {
        assert_eq!(
            mathml_to_latex("<msubsup><mi>x</mi><mi>i</mi><mn>2</mn></msubsup>"),
            "x_{i}^{2}"
        );
    }

    #[test]
    fn test_msqrt_no_degree() {
        assert_eq!(mathml_to_latex("<msqrt><mi>x</mi></msqrt>"), "\\sqrt{x}");
    }

    #[test]
    fn test_mroot_with_degree() {
        assert_eq!(mathml_to_latex("<mroot><mi>x</mi><mn>3</mn></mroot>"), "\\sqrt[3]{x}");
    }

    #[test]
    fn test_mfenced_default_parens() {
        assert_eq!(mathml_to_latex("<mfenced><mi>x</mi></mfenced>"), "\\left(x\\right)");
    }

    #[test]
    fn test_mfenced_brackets_multiple_elements() {
        assert_eq!(
            mathml_to_latex(r#"<mfenced open="[" close="]"><mi>a</mi><mi>b</mi></mfenced>"#),
            "\\left[a,b\\right]"
        );
    }

    #[test]
    fn test_munder() {
        assert_eq!(
            mathml_to_latex("<munder><mi>lim</mi><mi>n</mi></munder>"),
            "\\underset{n}{lim}"
        );
    }

    #[test]
    fn test_mover_hat_accent() {
        // A bare `^` inside `\overset` is unparseable LaTeX ("expected group
        // after ^"); accent characters must map to accent macros.
        assert_eq!(mathml_to_latex("<mover><mi>x</mi><mo>^</mo></mover>"), "\\hat{x}");
    }

    #[test]
    fn test_mover_accent_family() {
        assert_eq!(mathml_to_latex("<mover><mi>x</mi><mo>\u{02DC}</mo></mover>"), "\\tilde{x}");
        assert_eq!(mathml_to_latex("<mover><mi>q</mi><mo>\u{02D9}</mo></mover>"), "\\dot{q}");
        assert_eq!(mathml_to_latex("<mover><mi>y</mi><mo>\u{00AF}</mo></mover>"), "\\bar{y}");
        assert_eq!(mathml_to_latex("<mover><mi>v</mi><mo>\u{2192}</mo></mover>"), "\\vec{v}");
        // Multi-glyph base widens to the stretched forms.
        assert_eq!(
            mathml_to_latex("<mover><mrow><mi>a</mi><mi>b</mi></mrow><mo>\u{00AF}</mo></mover>"),
            "\\overline{ab}"
        );
    }

    #[test]
    fn test_munder_low_line_is_underline() {
        // Authors write lower bounds as `munder` with a low-line char.
        assert_eq!(mathml_to_latex("<munder><mi>m</mi><mo>_</mo></munder>"), "\\underline{m}");
    }

    #[test]
    fn test_mover_with_content_script_keeps_overset() {
        assert_eq!(
            mathml_to_latex("<mover><mi>x</mi><mi>n</mi></mover>"),
            "\\overset{n}{x}"
        );
    }

    #[test]
    fn test_literal_stretchy_brace_is_escaped() {
        // A `<mo>{</mo>` cases brace passed through raw changes LaTeX grouping
        // structure and leaves the formula unbalanced when its mate sits in
        // another table row.
        assert_eq!(mathml_to_latex("<mo>{</mo><mi>x</mi>"), "\\{x");
    }

    #[test]
    fn test_literal_backslash_is_escaped() {
        // Set difference written as a raw backslash: `A\B` must not fuse into
        // an undefined control sequence `\B`.
        assert_eq!(mathml_to_latex("<mi>A</mi><mo>\\</mo><mi>B</mi>"), "A\\backslash B");
    }

    #[test]
    fn test_mfenced_norm_delimiters() {
        assert_eq!(
            mathml_to_latex(r#"<mfenced open="&#x2225;" close="&#x2225;"><mi>x</mi></mfenced>"#),
            "\\left\\|x\\right\\|"
        );
    }

    #[test]
    fn test_mfenced_angle_delimiters_do_not_glue() {
        assert_eq!(
            mathml_to_latex(r#"<mfenced open="&#x27E8;" close="&#x27E9;"><mi>A</mi></mfenced>"#),
            "\\left\\langle A\\right\\rangle "
        );
    }

    #[test]
    fn test_mfenced_with_operator_children_drops_separators() {
        // `mfenced` abused as grouping: `(1 - x)` must not become `(1,-,x)`.
        assert_eq!(
            mathml_to_latex(r#"<mfenced><mn>1</mn><mo>-</mo><mi>x</mi></mfenced>"#),
            "\\left(1-x\\right)"
        );
    }

    #[test]
    fn test_mtext_greek_moves_outside_text_group() {
        // `\Delta` is math-mode-only; inside `\text{}` it is undefined.
        assert_eq!(
            mathml_to_latex("<mtext>rate \u{0394}x</mtext>"),
            "\\text{rate }\\Delta \\text{x}"
        );
    }

    #[test]
    fn test_mtext_escapes_structural_chars() {
        assert_eq!(
            mathml_to_latex("<mtext>m_{0} 50%</mtext>"),
            "\\text{m\\_\\{0\\} 50\\%}"
        );
    }

    #[test]
    fn test_braced_base_with_script_still_wraps() {
        // `{S_{\sigma }}_{1}` starts and ends with braces but is two atoms;
        // scripting it again without a wrap is a double subscript.
        assert_eq!(
            mathml_to_latex(
                "<msub><msub><mrow><mi>S</mi><mi>b</mi></mrow><mn>1</mn></msub><mn>2</mn></msub>"
            ),
            "{{Sb}_{1}}_{2}"
        );
    }

    #[test]
    fn test_scripted_base_wraps_before_outer_script() {
        // `\lambda _{1}^{'}` scripted again must brace-wrap, or the outer
        // script produces a double superscript.
        assert_eq!(
            mathml_to_latex(
                "<msup><msup><mi>\u{03BB}</mi><mn>1</mn></msup><mn>2</mn></msup>"
            ),
            "{\\lambda ^{1}}^{2}"
        );
    }

    #[test]
    fn test_empty_script_base_renders_as_empty_group() {
        // Tensor prescript markup: an empty base must yield `{}` so the script
        // cannot fuse onto the preceding atom as a double subscript.
        assert_eq!(
            mathml_to_latex("<msup><mi>T</mi><mi>\u{03BD}</mi></msup><msub><mrow/><mi>\u{03BD}</mi></msub>"),
            "T^{\\nu }{}_{\\nu }"
        );
    }

    #[test]
    fn test_combining_overline_folds_into_bar() {
        // Identifiers carry combining marks (`U̅`); the raw mark is not a
        // KaTeX-valid accent.
        assert_eq!(mathml_to_latex("<mi>U\u{0305}</mi>"), "\\bar{U}");
        // A mark split into its own element applies to the previous atom.
        assert_eq!(mathml_to_latex("<mi>\u{03A3}</mi><mo>\u{0305}</mo>"), "\\bar{\\Sigma} ");
    }

    #[test]
    fn test_munderover() {
        assert_eq!(
            mathml_to_latex("<munderover><mo>\u{2211}</mo><mi>i</mi><mi>n</mi></munderover>"),
            "\\overset{n}{\\underset{i}{\\sum }}"
        );
    }

    #[test]
    fn test_mspace_renders_as_space() {
        assert_eq!(mathml_to_latex("<mrow><mi>a</mi><mspace/><mi>b</mi></mrow>"), "a b");
    }

    #[test]
    fn test_mphantom() {
        assert_eq!(mathml_to_latex("<mphantom><mi>x</mi></mphantom>"), "\\phantom{x}");
    }

    #[test]
    fn test_mtable_matrix() {
        let latex = mathml_to_latex(
            r#"<mtable>
                <mtr><mtd><mn>1</mn></mtd><mtd><mn>2</mn></mtd></mtr>
                <mtr><mtd><mn>3</mn></mtd><mtd><mn>4</mn></mtd></mtr>
            </mtable>"#,
        );
        assert_eq!(latex, "\\begin{matrix}1 & 2 \\\\ 3 & 4\\end{matrix}");
    }

    #[test]
    fn test_unknown_element_degrades_to_text_content() {
        assert_eq!(mathml_to_latex("<mlongdiv><mn>42</mn></mlongdiv>"), "42");
    }

    #[test]
    fn test_semantics_renders_presentation_branch_only() {
        let latex = mathml_to_latex(
            r#"<semantics>
                <mrow><mi>E</mi><mo>=</mo><mi>m</mi></mrow>
                <annotation encoding="StarMath 5.0">E = m</annotation>
            </semantics>"#,
        );
        assert_eq!(latex, "E=m");
    }

    #[test]
    fn test_nested_quadratic_formula() {
        // Uses a literal '±' (U+00B1) character, matching how the OMML test
        // suite embeds Unicode math symbols directly rather than as escapes
        // (escape sequences are not processed inside raw strings).
        let latex = mathml_to_latex(
            r#"<mi>x</mi><mo>=</mo>
            <mfrac>
                <mrow>
                    <mo>-</mo><mi>b</mi><mo>±</mo>
                    <msqrt>
                        <msup><mi>b</mi><mn>2</mn></msup>
                        <mo>-</mo><mn>4</mn><mi>a</mi><mi>c</mi>
                    </msqrt>
                </mrow>
                <mrow><mn>2</mn><mi>a</mi></mrow>
            </mfrac>"#,
        );
        assert_eq!(latex, "x=\\frac{-b\\pm \\sqrt{b^{2}-4ac}}{2a}");
    }

    #[test]
    fn test_formula_odt_fixture_shape() {
        // Mirrors the real embedded formula object in test_documents/odt/formula.odt:
        // E = m * c^2, wrapped in <semantics>/<annotation> with a StarMath fallback.
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
            <math xmlns="http://www.w3.org/1998/Math/MathML">
                <semantics>
                    <mrow><mrow><mi>E</mi><mo stretchy="false">=</mo>
                    <mrow><mi>m</mi><mo stretchy="false">⋅</mo>
                    <msup><mi>c</mi><mn>2</mn></msup></mrow></mrow></mrow>
                    <annotation encoding="StarMath 5.0">E = m cdot c^2</annotation>
                </semantics>
            </math>"#;
        let mut budget = SecurityBudget::with_defaults();
        let latex = convert_mathml_str_to_latex(xml, &mut budget).expect("conversion ok");
        assert_eq!(latex, "E=m\\cdot c^{2}");
    }

    #[test]
    fn test_convert_mathml_node_to_latex_from_pre_parsed_node() {
        let xml = r#"<math xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math>"#;
        let doc = roxmltree::Document::parse(xml).expect("parses");
        let mut budget = SecurityBudget::with_defaults();
        let latex = convert_mathml_node_to_latex(doc.root_element(), &mut budget).expect("conversion ok");
        assert_eq!(latex, "x");
    }
}
