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

/// `annotation` encodings whose text is the LaTeX the author wrote.
///
/// A document that ships one states the formula exactly, so it beats
/// reconstructing LaTeX from the presentation tree, which can only approximate
/// the author's spelling.
const TEX_ANNOTATION_ENCODINGS: &[&str] = &["application/x-tex", "text/x-tex", "tex", "latex"];

/// Names of MathML elements that are pure grouping/styling wrappers: their
/// children are rendered in sequence with no LaTeX markup of their own.
const TRANSPARENT_ELEMENTS: &[&str] = &["math", "mrow", "mstyle", "mpadded", "merror"];

#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
enum MmlNode {
    /// LaTeX taken verbatim from a TeX `annotation`.
    Verbatim(String),
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

/// The LaTeX of a `semantics` child annotation that carries TeX, if any.
///
/// Renderers wrap the whole expression in `{\displaystyle ...}` or
/// `{\textstyle ...}` to state the style the surrounding document set. That
/// wrapper is presentation, not the formula, so it comes off; `$` delimiters
/// come off for the same reason the projection strips them.
fn tex_annotation(node: Node, budget: &mut SecurityBudget) -> Result<Option<String>, SecurityError> {
    for child in node.children().filter(|c| c.is_element()) {
        if !child.tag_name().name().eq_ignore_ascii_case("annotation") {
            continue;
        }
        let Some(encoding) = child.attribute("encoding") else {
            continue;
        };
        if !TEX_ANNOTATION_ENCODINGS
            .iter()
            .any(|known| known.eq_ignore_ascii_case(encoding.trim()))
        {
            continue;
        }
        let text = collect_text(child, budget)?;
        let latex = strip_style_wrapper(crate::extraction::derive::strip_math_delimiters(text.trim()));
        if !latex.is_empty() {
            return Ok(Some(latex.to_string()));
        }
    }
    Ok(None)
}

/// Remove a `{\displaystyle ...}` or `{\textstyle ...}` wrapper around the
/// whole expression.
fn strip_style_wrapper(latex: &str) -> &str {
    for prefix in ["{\\displaystyle", "{\\textstyle", "{\\scriptstyle"] {
        let Some(rest) = latex.strip_prefix(prefix) else {
            continue;
        };
        let Some(inner) = rest.strip_suffix('}') else {
            continue;
        };
        // The wrapper must enclose everything: a brace that closes early means
        // the tail belongs to the formula.
        let mut depth = 1i32;
        for ch in inner.chars() {
            match ch {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                return latex;
            }
        }
        return inner.trim();
    }
    latex
}

/// The LaTeX of a content-MathML `annotation-xml` child, if the element has one.
fn content_annotation(node: Node, budget: &mut SecurityBudget) -> Result<Option<String>, SecurityError> {
    for child in node.children().filter(|c| c.is_element()) {
        if !child.tag_name().name().eq_ignore_ascii_case("annotation-xml") {
            continue;
        }
        let is_content = child
            .attribute("encoding")
            .is_some_and(|e| e.trim().eq_ignore_ascii_case("MathML-Content"));
        if !is_content {
            continue;
        }
        for inner in child.children().filter(|c| c.is_element()) {
            let latex = convert_content_node(inner, budget)?;
            if !latex.trim().is_empty() {
                return Ok(Some(latex.trim().to_string()));
            }
        }
    }
    Ok(None)
}

/// Content MathML operators that render as an infix chain.
const INFIX_OPERATORS: &[(&str, &str)] = &[
    ("plus", "+"),
    ("minus", "-"),
    ("times", " \\times "),
    ("divide", " \\div "),
    ("eq", "="),
    ("neq", " \\ne "),
    ("lt", " < "),
    ("gt", " > "),
    ("leq", " \\le "),
    ("geq", " \\ge "),
    ("equivalent", " \\equiv "),
    ("approx", " \\approx "),
    ("and", " \\land "),
    ("or", " \\lor "),
    ("implies", " \\implies "),
    ("in", " \\in "),
    ("notin", " \\notin "),
    ("subset", " \\subset "),
    ("prsubset", " \\subsetneq "),
    ("union", " \\cup "),
    ("intersect", " \\cap "),
    ("setdiff", " \\setminus "),
    ("cartesianproduct", " \\times "),
    ("compose", " \\circ "),
];

/// Content MathML operators that render as a named LaTeX function.
const FUNCTION_OPERATORS: &[(&str, &str)] = &[
    ("sin", "\\sin"),
    ("cos", "\\cos"),
    ("tan", "\\tan"),
    ("sec", "\\sec"),
    ("csc", "\\csc"),
    ("cot", "\\cot"),
    ("arcsin", "\\arcsin"),
    ("arccos", "\\arccos"),
    ("arctan", "\\arctan"),
    ("sinh", "\\sinh"),
    ("cosh", "\\cosh"),
    ("tanh", "\\tanh"),
    ("exp", "\\exp"),
    ("ln", "\\ln"),
    ("log", "\\log"),
    ("det", "\\det"),
    ("gcd", "\\gcd"),
    ("max", "\\max"),
    ("min", "\\min"),
];

/// Convert a content-MathML `apply` subtree to LaTeX.
///
/// Content MathML states what a formula *means* (`<apply><plus/><ci>a</ci>…`)
/// rather than how it looks, so it converts by operator rather than by layout.
/// An operator with no LaTeX spelling becomes `\operatorname{name}(args)`, which
/// parses and still names what the source said.
fn convert_apply(node: Node, budget: &mut SecurityBudget) -> Result<String, SecurityError> {
    budget.step()?;
    let mut children = node.children().filter(|c| c.is_element());
    let Some(operator) = children.next() else {
        return Ok(String::new());
    };
    let name = operator.tag_name().name().to_ascii_lowercase();

    // `bvar`, `lowlimit`, `uplimit`, `degree`, and `condition` qualify the
    // operator; everything else is an operand.
    let mut operands: Vec<Node> = Vec::new();
    let (mut bvar, mut lower, mut upper, mut degree) = (None, None, None, None);
    for child in children {
        match child.tag_name().name().to_ascii_lowercase().as_str() {
            "bvar" => bvar = Some(child),
            "lowlimit" | "condition" => lower = Some(child),
            "uplimit" => upper = Some(child),
            "degree" => degree = Some(child),
            _ => operands.push(child),
        }
    }

    let rendered: Vec<String> = operands
        .iter()
        .map(|c| convert_content_node(*c, budget))
        .collect::<Result<_, _>>()?;
    let qualifier = |q: Option<Node>, budget: &mut SecurityBudget| -> Result<String, SecurityError> {
        match q {
            Some(n) => {
                let parts: Vec<String> = n
                    .children()
                    .filter(|c| c.is_element())
                    .map(|c| convert_content_node(c, budget))
                    .collect::<Result<_, _>>()?;
                Ok(parts.join(""))
            }
            None => Ok(String::new()),
        }
    };

    if let Some((_, latex)) = INFIX_OPERATORS.iter().find(|(op, _)| *op == name) {
        // Unary minus reads as negation rather than subtraction.
        if name == "minus" && rendered.len() == 1 {
            return Ok(format!("-{}", rendered[0]));
        }
        return Ok(rendered.join(latex));
    }
    if let Some((_, latex)) = FUNCTION_OPERATORS.iter().find(|(op, _)| *op == name) {
        return Ok(format!("{latex}\\left({}\\right)", rendered.join(", ")));
    }

    match name.as_str() {
        "power" if rendered.len() == 2 => Ok(format!("{}^{{{}}}", rendered[0], rendered[1])),
        "root" => {
            let index = qualifier(degree, budget)?;
            let radicand = rendered.first().cloned().unwrap_or_default();
            if index.is_empty() || index == "2" {
                Ok(format!("\\sqrt{{{radicand}}}"))
            } else {
                Ok(format!("\\sqrt[{index}]{{{radicand}}}"))
            }
        }
        "abs" => Ok(format!("\\left|{}\\right|", rendered.join(", "))),
        "floor" => Ok(format!("\\lfloor {}\\rfloor", rendered.join(", "))),
        "ceiling" => Ok(format!("\\lceil {}\\rceil", rendered.join(", "))),
        "factorial" => Ok(format!("{}!", rendered.join(""))),
        "sum" | "product" | "int" => {
            let command = match name.as_str() {
                "sum" => "\\sum",
                "product" => "\\prod",
                _ => "\\int",
            };
            let var = qualifier(bvar, budget)?;
            let from = qualifier(lower, budget)?;
            let to = qualifier(upper, budget)?;
            let mut out = String::from(command);
            if !from.is_empty() {
                let start = if var.is_empty() { from } else { format!("{var}={from}") };
                out.push_str(&format!("_{{{start}}}"));
            } else if !var.is_empty() {
                out.push_str(&format!("_{{{var}}}"));
            }
            if !to.is_empty() {
                out.push_str(&format!("^{{{to}}}"));
            }
            out.push(' ');
            out.push_str(&rendered.join(""));
            if name == "int" && !var.is_empty() {
                out.push_str(&format!("\\,d{var}"));
            }
            Ok(out)
        }
        "diff" => {
            let var = qualifier(bvar, budget)?;
            let body = rendered.join("");
            if var.is_empty() {
                Ok(format!("\\frac{{d}}{{dx}}{body}"))
            } else {
                Ok(format!("\\frac{{d}}{{d{var}}}{body}"))
            }
        }
        // An operator the mapping does not name still parses and still says what
        // the source said.
        _ => Ok(format!("\\operatorname{{{}}}\\left({}\\right)", name, rendered.join(", "))),
    }
}

/// Convert one content-MathML node to LaTeX.
fn convert_content_node(node: Node, budget: &mut SecurityBudget) -> Result<String, SecurityError> {
    budget.step()?;
    match node.tag_name().name().to_ascii_lowercase().as_str() {
        "apply" => convert_apply(node, budget),
        "ci" | "cn" | "csymbol" => {
            let text = collect_text(node, budget)?;
            let mut out = String::new();
            crate::extraction::math_symbols::render_run_text(text.trim(), &mut out);
            Ok(out)
        }
        "matrix" | "vector" => {
            let rows: Vec<String> = node
                .children()
                .filter(|c| c.is_element())
                .map(|row| {
                    let cells: Vec<String> = row
                        .children()
                        .filter(|c| c.is_element())
                        .map(|c| convert_content_node(c, budget))
                        .collect::<Result<_, _>>()?;
                    Ok(if cells.is_empty() {
                        convert_content_node(row, budget)?
                    } else {
                        cells.join(" & ")
                    })
                })
                .collect::<Result<_, SecurityError>>()?;
            Ok(format!("\\begin{{pmatrix}}{}\\end{{pmatrix}}", rows.join(" \\\\ ")))
        }
        "piecewise" => {
            let mut rows: Vec<String> = Vec::new();
            for piece in node.children().filter(|c| c.is_element()) {
                let parts: Vec<String> = piece
                    .children()
                    .filter(|c| c.is_element())
                    .map(|c| convert_content_node(c, budget))
                    .collect::<Result<_, _>>()?;
                rows.push(match piece.tag_name().name().to_ascii_lowercase().as_str() {
                    "otherwise" => format!("{} & \\text{{otherwise}}", parts.join("")),
                    _ => parts.join(" & \\text{if }"),
                });
            }
            Ok(format!("\\begin{{cases}}{}\\end{{cases}}", rows.join(" \\\\ ")))
        }
        "list" | "set" => {
            let items: Vec<String> = node
                .children()
                .filter(|c| c.is_element())
                .map(|c| convert_content_node(c, budget))
                .collect::<Result<_, _>>()?;
            let inner = items.join(", ");
            Ok(if node.tag_name().name().eq_ignore_ascii_case("set") {
                format!("\\{{{inner}\\}}")
            } else {
                format!("\\left({inner}\\right)")
            })
        }
        // A constant such as `<pi/>` or `<exponentiale/>` carries its meaning in
        // its name.
        "" => Ok(String::new()),
        other => match other {
            "pi" => Ok("\\pi".to_string()),
            "exponentiale" => Ok("e".to_string()),
            "imaginaryi" => Ok("i".to_string()),
            "infinity" => Ok("\\infty".to_string()),
            "true" => Ok("\\text{true}".to_string()),
            "false" => Ok("\\text{false}".to_string()),
            "emptyset" => Ok("\\emptyset".to_string()),
            _ => {
                let text = collect_text(node, budget)?;
                Ok(text.trim().to_string())
            }
        },
    }
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
        // Content MathML states meaning rather than layout, so it converts by
        // operator. It appears as a `math` child in content documents and inside
        // `annotation-xml` in mixed ones.
        "apply" | "piecewise" | "matrix" | "vector" | "set" | "list" => {
            Ok(MmlNode::Verbatim(convert_content_node(node, budget)?))
        }
        "ci" | "cn" | "csymbol" => Ok(MmlNode::Verbatim(convert_content_node(node, budget)?)),
        "semantics" => {
            if let Some(tex) = tex_annotation(node, budget)? {
                return Ok(MmlNode::Verbatim(tex));
            }
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
            // A document may carry only the content branch, in which case the
            // presentation side renders to nothing and the meaning is all there
            // is to work from.
            if render_nodes(&children).trim().is_empty()
                && let Some(latex) = content_annotation(node, budget)?
            {
                return Ok(MmlNode::Verbatim(latex));
            }
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
        MmlNode::Verbatim(latex) => out.push_str(latex),
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

    /// A document that ships the author's TeX states the formula exactly, so it
    /// beats reconstructing LaTeX from the presentation tree.
    #[test]
    fn test_tex_annotation_wins_over_the_presentation_tree() {
        let latex = mathml_to_latex(
            r#"<semantics>
                <mrow><mi>E</mi><mo>=</mo><mi>m</mi><msup><mi>c</mi><mn>2</mn></msup></mrow>
                <annotation encoding="application/x-tex">E = mc^2</annotation>
            </semantics>"#,
        );
        assert_eq!(latex, "E = mc^2");
    }

    /// Renderers wrap the expression in the style the surrounding document set.
    #[test]
    fn test_display_style_wrapper_comes_off() {
        let latex = mathml_to_latex(
            r#"<semantics>
                <mrow><mi>x</mi></mrow>
                <annotation encoding="application/x-tex">{\displaystyle x^{2}+1}</annotation>
            </semantics>"#,
        );
        assert_eq!(latex, "x^{2}+1");
    }

    /// A brace that closes before the end is part of the formula, so the
    /// wrapper stays.
    #[test]
    fn test_partial_brace_group_keeps_the_wrapper() {
        assert_eq!(
            strip_style_wrapper("{\\displaystyle a} + b"),
            "{\\displaystyle a} + b"
        );
    }

    /// An annotation in another notation is not TeX and must not leak.
    #[test]
    fn test_non_tex_annotation_still_renders_the_presentation_branch() {
        let latex = mathml_to_latex(
            r#"<semantics>
                <mrow><mi>E</mi><mo>=</mo><mi>m</mi></mrow>
                <annotation encoding="StarMath 5.0">E = m</annotation>
            </semantics>"#,
        );
        assert_eq!(latex, "E=m");
    }

    /// An empty annotation carries nothing, so the presentation tree stands.
    #[test]
    fn test_empty_tex_annotation_falls_back() {
        let latex = mathml_to_latex(
            r#"<semantics>
                <mrow><mi>a</mi><mo>+</mo><mi>b</mi></mrow>
                <annotation encoding="application/x-tex">   </annotation>
            </semantics>"#,
        );
        assert_eq!(latex, "a+b");
    }

    /// Content MathML states meaning rather than layout, so an `apply` tree
    /// converts by operator.
    #[test]
    fn test_content_mathml_apply_converts_by_operator() {
        assert_eq!(
            mathml_to_latex("<apply><plus/><ci>a</ci><ci>b</ci></apply>"),
            "a+b"
        );
        assert_eq!(
            mathml_to_latex("<apply><power/><ci>x</ci><cn>2</cn></apply>"),
            "x^{2}"
        );
        assert_eq!(
            mathml_to_latex("<apply><root/><degree><cn>3</cn></degree><ci>x</ci></apply>"),
            "\\sqrt[3]{x}"
        );
    }

    #[test]
    fn test_content_mathml_functions_and_relations() {
        assert_eq!(
            mathml_to_latex("<apply><eq/><ci>y</ci><apply><sin/><ci>x</ci></apply></apply>"),
            "y=\\sin\\left(x\\right)"
        );
    }

    /// A sum carries its bound variable and limits.
    #[test]
    fn test_content_mathml_sum_carries_limits() {
        let latex = mathml_to_latex(
            "<apply><sum/><bvar><ci>i</ci></bvar><lowlimit><cn>1</cn></lowlimit>             <uplimit><ci>n</ci></uplimit><ci>i</ci></apply>",
        );
        assert_eq!(latex, "\\sum_{i=1}^{n} i");
    }

    #[test]
    fn test_content_mathml_matrix_and_piecewise() {
        let matrix = mathml_to_latex(
            "<matrix><matrixrow><cn>1</cn><cn>0</cn></matrixrow><matrixrow><cn>0</cn><cn>1</cn></matrixrow></matrix>",
        );
        assert_eq!(matrix, "\\begin{pmatrix}1 & 0 \\\\ 0 & 1\\end{pmatrix}");

        let cases = mathml_to_latex(
            "<piecewise><piece><cn>0</cn><apply><lt/><ci>x</ci><cn>0</cn></apply></piece>             <otherwise><ci>x</ci></otherwise></piecewise>",
        );
        assert!(cases.starts_with("\\begin{cases}"), "got: {cases}");
        assert!(cases.contains("\\text{otherwise}"), "got: {cases}");
    }

    /// An operator the mapping does not name still parses and still says what
    /// the source said.
    #[test]
    fn test_unknown_content_operator_degrades() {
        assert_eq!(
            mathml_to_latex("<apply><wibble/><ci>a</ci></apply>"),
            "\\operatorname{wibble}\\left(a\\right)"
        );
    }

    /// A document that carries only the content branch has its meaning read from
    /// `annotation-xml`, since the presentation side renders to nothing.
    #[test]
    fn test_content_annotation_is_used_when_presentation_is_empty() {
        let latex = mathml_to_latex(
            r#"<semantics><mrow/><annotation-xml encoding="MathML-Content">
                <apply><plus/><ci>a</ci><ci>b</ci></apply>
            </annotation-xml></semantics>"#,
        );
        assert_eq!(latex, "a+b");
    }

    /// A document with a working presentation branch keeps using it, so nothing
    /// that already converted changes.
    #[test]
    fn test_presentation_branch_still_wins_over_content_annotation() {
        let latex = mathml_to_latex(
            r#"<semantics><mrow><mi>E</mi><mo>=</mo><mi>m</mi></mrow>
                <annotation-xml encoding="MathML-Content"><apply><plus/><ci>q</ci><ci>r</ci></apply></annotation-xml>
            </semantics>"#,
        );
        assert_eq!(latex, "E=m");
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
