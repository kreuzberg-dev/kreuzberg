//! OMML (Office Math Markup Language) to LaTeX converter.
//!
//! Converts OMML math elements found in DOCX files to LaTeX notation.
//! When the streaming parser encounters `m:oMathPara` or `m:oMath`, it
//! delegates here. We collect the subtree into a `MathNode` tree, then
//! recursively render it to LaTeX.

use crate::extraction::math_symbols::render_run_text;
use crate::extractors::security::{SecurityBudget, SecurityError};
use quick_xml::Reader;
use quick_xml::events::Event;

#[derive(Debug, Clone)]
enum FracType {
    Bar,
    NoBar,
    Linear,
    Skewed,
}

#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
enum MathNode {
    /// Plain text from m:r/m:t
    Run(String),
    /// Superscript: base^{sup}
    SSup { base: Vec<MathNode>, sup: Vec<MathNode> },
    /// Subscript: base_{sub}
    SSub { base: Vec<MathNode>, sub: Vec<MathNode> },
    /// Sub-superscript: base_{sub}^{sup}
    SSubSup {
        base: Vec<MathNode>,
        sub: Vec<MathNode>,
        sup: Vec<MathNode>,
    },
    /// Fraction: \frac{num}{den}
    Frac {
        num: Vec<MathNode>,
        den: Vec<MathNode>,
        frac_type: FracType,
    },
    /// Radical: \sqrt{body} or \sqrt[deg]{body}
    Rad {
        deg: Vec<MathNode>,
        body: Vec<MathNode>,
        deg_hide: bool,
    },
    /// N-ary operator: \sum_{sub}^{sup}{body}
    Nary {
        chr: String,
        sub: Vec<MathNode>,
        sup: Vec<MathNode>,
        body: Vec<MathNode>,
        sub_hide: bool,
        sup_hide: bool,
    },
    /// Delimiter: \left( ... \right)
    Delim {
        begin_chr: String,
        end_chr: String,
        sep_chr: String,
        elements: Vec<Vec<MathNode>>,
    },
    /// Function: \funcname{body}
    Func { name: Vec<MathNode>, body: Vec<MathNode> },
    /// Accent: \hat{body}
    Acc { chr: String, body: Vec<MathNode> },
    /// Equation array: \begin{aligned}...\end{aligned}
    EqArr { rows: Vec<Vec<MathNode>> },
    /// Lower limit: \underset{lim}{body}
    LimLow { body: Vec<MathNode>, lim: Vec<MathNode> },
    /// Upper limit: \overset{lim}{body}
    LimUpp { body: Vec<MathNode>, lim: Vec<MathNode> },
    /// Bar (overline/underline)
    Bar { body: Vec<MathNode>, top: bool },
    /// Border box: \boxed{body}
    BorderBox { body: Vec<MathNode> },
    /// Matrix: \begin{matrix}...\end{matrix}
    Matrix { rows: Vec<Vec<Vec<MathNode>>> },
    /// Grouping container (m:box, m:phant, etc.) — passes through children
    Group { children: Vec<MathNode> },
    /// Pre-sub-superscript: {}_{sub}^{sup}{base}
    SPre {
        base: Vec<MathNode>,
        sub: Vec<MathNode>,
        sup: Vec<MathNode>,
    },
}

/// Collect an `m:oMathPara` subtree and convert to LaTeX (display math).
/// The reader should be positioned right after the `<m:oMathPara>` start tag.
pub(crate) fn collect_and_convert_omath_para(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<String, SecurityError> {
    let children = collect_children(reader, b"m:oMathPara", budget)?;
    let mut parts = Vec::new();
    for child in &children {
        if let MathNode::Group { children: inner } = child {
            let rendered = render_nodes(inner);
            if !rendered.is_empty() {
                parts.push(rendered);
            }
        }
    }
    if parts.is_empty() {
        Ok(render_nodes(&children))
    } else {
        Ok(parts.join(" \\\\ "))
    }
}

/// Collect an `m:oMath` subtree and convert to LaTeX (inline math).
/// The reader should be positioned right after the `<m:oMath>` start tag.
pub(crate) fn collect_and_convert_omath(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<String, SecurityError> {
    let children = collect_children(reader, b"m:oMath", budget)?;
    Ok(render_nodes(&children))
}

/// Recursively collect child nodes until the matching close tag.
fn collect_children(
    reader: &mut Reader<&[u8]>,
    end_tag: &[u8],
    budget: &mut SecurityBudget,
) -> Result<Vec<MathNode>, SecurityError> {
    let mut nodes = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                let tag = (e.name().as_ref() as &[u8]).to_vec();
                match tag.as_slice() {
                    b"m:r" => {
                        nodes.push(collect_run(reader, budget)?);
                    }
                    b"m:sSup" => {
                        nodes.push(collect_ssup(reader, budget)?);
                    }
                    b"m:sSub" => {
                        nodes.push(collect_ssub(reader, budget)?);
                    }
                    b"m:sSubSup" => {
                        nodes.push(collect_ssubsup(reader, budget)?);
                    }
                    b"m:f" => {
                        nodes.push(collect_frac(reader, budget)?);
                    }
                    b"m:rad" => {
                        nodes.push(collect_rad(reader, budget)?);
                    }
                    b"m:nary" => {
                        nodes.push(collect_nary(reader, budget)?);
                    }
                    b"m:d" => {
                        nodes.push(collect_delim(reader, budget)?);
                    }
                    b"m:func" => {
                        nodes.push(collect_func(reader, budget)?);
                    }
                    b"m:acc" => {
                        nodes.push(collect_acc(reader, budget)?);
                    }
                    b"m:eqArr" => {
                        nodes.push(collect_eqarr(reader, budget)?);
                    }
                    b"m:limLow" => {
                        nodes.push(collect_limlow(reader, budget)?);
                    }
                    b"m:limUpp" => {
                        nodes.push(collect_limupp(reader, budget)?);
                    }
                    b"m:bar" => {
                        nodes.push(collect_bar(reader, budget)?);
                    }
                    b"m:borderBox" => {
                        nodes.push(collect_borderbox(reader, budget)?);
                    }
                    b"m:m" => {
                        nodes.push(collect_matrix(reader, budget)?);
                    }
                    b"m:box" | b"m:phant" => {
                        let children = collect_element_body(reader, &tag, budget)?;
                        nodes.push(MathNode::Group { children });
                    }
                    b"m:sPre" => {
                        nodes.push(collect_spre(reader, budget)?);
                    }
                    b"m:oMath" => {
                        let inner = collect_children(reader, b"m:oMath", budget)?;
                        nodes.push(MathNode::Group { children: inner });
                    }
                    _ => {
                        // `skip_to_end` reads through its own matching end tag directly,
                        // so the `Event::End` arm below never sees it. Refund the enter
                        // above or every unrecognized OMML tag leaks one depth level.
                        skip_to_end(reader, &tag);
                        budget.leave();
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == end_tag {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(nodes)
}

/// Collect text from an m:r element (reads until </m:r>).
fn collect_run(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut text = String::new();
    let mut buf = Vec::new();
    let mut in_text = false;

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:t" => in_text = true,
                    b"m:rPr" => {
                        // Consumes its own `</m:rPr>`; refund the enter above.
                        skip_to_end(reader, b"m:rPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_text && let Ok(t) = e.decode() {
                    budget.check_entity(&t)?;
                    budget.account_text(t.len())?;
                    text.push_str(&t);
                }
            }
            Ok(Event::GeneralRef(ref e)) if in_text => {
                let t = crate::utils::xml_utils::resolve_general_ref(e);
                budget.account_text(t.len())?;
                text.push_str(&t);
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                match e.name().as_ref() as &[u8] {
                    b"m:t" => in_text = false,
                    b"m:r" => break,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Run(text))
}

/// Collect an m:sSup (superscript) element.
fn collect_ssup(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut base = Vec::new();
    let mut sup = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => base = collect_children(reader, b"m:e", budget)?,
                    b"m:sup" => sup = collect_children(reader, b"m:sup", budget)?,
                    b"m:sSupPr" => {
                        // Consumes its own `</m:sSupPr>`; refund the enter above.
                        skip_to_end(reader, b"m:sSupPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:sSup" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::SSup { base, sup })
}

/// Collect an m:sSub (subscript) element.
fn collect_ssub(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut base = Vec::new();
    let mut sub = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => base = collect_children(reader, b"m:e", budget)?,
                    b"m:sub" => sub = collect_children(reader, b"m:sub", budget)?,
                    b"m:sSubPr" => {
                        // Consumes its own `</m:sSubPr>`; refund the enter above.
                        skip_to_end(reader, b"m:sSubPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:sSub" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::SSub { base, sub })
}

/// Collect an m:sSubSup element.
fn collect_ssubsup(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut base = Vec::new();
    let mut sub = Vec::new();
    let mut sup = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => base = collect_children(reader, b"m:e", budget)?,
                    b"m:sub" => sub = collect_children(reader, b"m:sub", budget)?,
                    b"m:sup" => sup = collect_children(reader, b"m:sup", budget)?,
                    b"m:sSubSupPr" => {
                        // Consumes its own `</m:sSubSupPr>`; refund the enter above.
                        skip_to_end(reader, b"m:sSubSupPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:sSubSup" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::SSubSup { base, sub, sup })
}

/// Collect an m:f (fraction) element.
fn collect_frac(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut num = Vec::new();
    let mut den = Vec::new();
    let mut frac_type = FracType::Bar;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:fPr" => {
                        // `collect_frac_pr` reads through its own `</m:fPr>` without
                        // calling `budget.leave()`; refund the enter above.
                        frac_type = collect_frac_pr(reader, budget)?;
                        budget.leave();
                    }
                    b"m:num" => num = collect_children(reader, b"m:num", budget)?,
                    b"m:den" => den = collect_children(reader, b"m:den", budget)?,
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:f" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Frac { num, den, frac_type })
}

/// Read fraction properties to determine type.
fn collect_frac_pr(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<FracType, SecurityError> {
    let mut frac_type = FracType::Bar;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                if e.name().as_ref() as &[u8] == b"m:type"
                    && let Some(val) = get_m_val(e)
                {
                    frac_type = match val.as_str() {
                        "noBar" => FracType::NoBar,
                        "lin" => FracType::Linear,
                        "skw" => FracType::Skewed,
                        _ => FracType::Bar,
                    };
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() as &[u8] == b"m:fPr" => {
                break;
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(frac_type)
}

/// Collect an m:rad (radical/sqrt) element.
fn collect_rad(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut deg = Vec::new();
    let mut body = Vec::new();
    let mut deg_hide = true;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:radPr" => {
                        // `collect_rad_pr` reads through its own `</m:radPr>` without
                        // calling `budget.leave()`; refund the enter above.
                        deg_hide = collect_rad_pr(reader, budget)?;
                        budget.leave();
                    }
                    b"m:deg" => deg = collect_children(reader, b"m:deg", budget)?,
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:rad" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Rad { deg, body, deg_hide })
}

/// Read radical properties (degHide).
fn collect_rad_pr(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<bool, SecurityError> {
    let mut deg_hide = true;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) if e.name().as_ref() as &[u8] == b"m:degHide" => {
                deg_hide = get_m_val(e).as_deref() != Some("0");
            }
            Ok(Event::End(ref e)) if e.name().as_ref() as &[u8] == b"m:radPr" => {
                break;
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(deg_hide)
}

/// Collect an m:nary (n-ary operator) element.
fn collect_nary(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut chr = "\u{222B}".to_string();
    let mut sub = Vec::new();
    let mut sup = Vec::new();
    let mut body = Vec::new();
    let mut sub_hide = false;
    let mut sup_hide = false;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:naryPr" => {
                        // `collect_nary_pr` reads through its own `</m:naryPr>` without
                        // calling `budget.leave()`; refund the enter above.
                        collect_nary_pr(reader, &mut chr, &mut sub_hide, &mut sup_hide, budget)?;
                        budget.leave();
                    }
                    b"m:sub" => sub = collect_children(reader, b"m:sub", budget)?,
                    b"m:sup" => sup = collect_children(reader, b"m:sup", budget)?,
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:nary" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Nary {
        chr,
        sub,
        sup,
        body,
        sub_hide,
        sup_hide,
    })
}

/// Read n-ary properties.
fn collect_nary_pr(
    reader: &mut Reader<&[u8]>,
    chr: &mut String,
    sub_hide: &mut bool,
    sup_hide: &mut bool,
    budget: &mut SecurityBudget,
) -> Result<(), SecurityError> {
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => match e.name().as_ref() as &[u8] {
                b"m:chr" => {
                    if let Some(val) = get_m_val(e) {
                        *chr = val;
                    }
                }
                b"m:subHide" => {
                    *sub_hide = get_m_val(e).as_deref() != Some("0");
                }
                b"m:supHide" => {
                    *sup_hide = get_m_val(e).as_deref() != Some("0");
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() as &[u8] == b"m:naryPr" => {
                break;
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

/// Collect an m:d (delimiter) element.
fn collect_delim(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut begin_chr = "(".to_string();
    let mut end_chr = ")".to_string();
    let mut sep_chr = "|".to_string();
    let mut elements: Vec<Vec<MathNode>> = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:dPr" => {
                        // `collect_delim_pr` reads through its own `</m:dPr>` without
                        // calling `budget.leave()`; refund the enter above.
                        collect_delim_pr(reader, &mut begin_chr, &mut end_chr, &mut sep_chr, budget)?;
                        budget.leave();
                    }
                    b"m:e" => {
                        elements.push(collect_children(reader, b"m:e", budget)?);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:d" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Delim {
        begin_chr,
        end_chr,
        sep_chr,
        elements,
    })
}

/// Read delimiter properties.
fn collect_delim_pr(
    reader: &mut Reader<&[u8]>,
    begin_chr: &mut String,
    end_chr: &mut String,
    sep_chr: &mut String,
    budget: &mut SecurityBudget,
) -> Result<(), SecurityError> {
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => match e.name().as_ref() as &[u8] {
                b"m:begChr" => {
                    if let Some(val) = get_m_val(e) {
                        *begin_chr = val;
                    }
                }
                b"m:endChr" => {
                    if let Some(val) = get_m_val(e) {
                        *end_chr = val;
                    }
                }
                b"m:sepChr" => {
                    if let Some(val) = get_m_val(e) {
                        *sep_chr = val;
                    }
                }
                _ => {}
            },
            Ok(Event::End(ref e)) if e.name().as_ref() as &[u8] == b"m:dPr" => {
                break;
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

/// Collect an m:func element.
fn collect_func(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut name = Vec::new();
    let mut body = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:fName" => name = collect_children(reader, b"m:fName", budget)?,
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    b"m:funcPr" => {
                        // Consumes its own `</m:funcPr>`; refund the enter above.
                        skip_to_end(reader, b"m:funcPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:func" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Func { name, body })
}

/// Collect an m:acc (accent) element.
fn collect_acc(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut chr = "\u{0302}".to_string();
    let mut body = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:accPr" => {
                        // `collect_acc_pr` reads through its own `</m:accPr>` without
                        // calling `budget.leave()`; refund the enter above.
                        collect_acc_pr(reader, &mut chr, budget)?;
                        budget.leave();
                    }
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:acc" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Acc { chr, body })
}

/// Read accent properties.
fn collect_acc_pr(
    reader: &mut Reader<&[u8]>,
    chr: &mut String,
    budget: &mut SecurityBudget,
) -> Result<(), SecurityError> {
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                if e.name().as_ref() as &[u8] == b"m:chr"
                    && let Some(val) = get_m_val(e)
                {
                    *chr = val;
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() as &[u8] == b"m:accPr" => {
                break;
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(())
}

/// Collect an m:eqArr element.
fn collect_eqarr(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut rows = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => rows.push(collect_children(reader, b"m:e", budget)?),
                    b"m:eqArrPr" => {
                        // Consumes its own `</m:eqArrPr>`; refund the enter above.
                        skip_to_end(reader, b"m:eqArrPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:eqArr" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::EqArr { rows })
}

/// Collect an m:limLow element.
fn collect_limlow(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut body = Vec::new();
    let mut lim = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    b"m:lim" => lim = collect_children(reader, b"m:lim", budget)?,
                    b"m:limLowPr" => {
                        // Consumes its own `</m:limLowPr>`; refund the enter above.
                        skip_to_end(reader, b"m:limLowPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:limLow" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::LimLow { body, lim })
}

/// Collect an m:limUpp element.
fn collect_limupp(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut body = Vec::new();
    let mut lim = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    b"m:lim" => lim = collect_children(reader, b"m:lim", budget)?,
                    b"m:limUppPr" => {
                        // Consumes its own `</m:limUppPr>`; refund the enter above.
                        skip_to_end(reader, b"m:limUppPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:limUpp" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::LimUpp { body, lim })
}

/// Collect an m:bar element.
fn collect_bar(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut body = Vec::new();
    let mut top = true;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:barPr" => {
                        // `collect_bar_pr` reads through its own `</m:barPr>` without
                        // calling `budget.leave()`; refund the enter above.
                        top = collect_bar_pr(reader, budget)?;
                        budget.leave();
                    }
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:bar" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Bar { body, top })
}

/// Read bar properties (pos).
fn collect_bar_pr(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<bool, SecurityError> {
    let mut top = true;
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e) | Event::Empty(ref e)) => {
                if e.name().as_ref() as &[u8] == b"m:pos"
                    && let Some(val) = get_m_val(e)
                {
                    top = val != "bot";
                }
            }
            Ok(Event::End(ref e)) if e.name().as_ref() as &[u8] == b"m:barPr" => {
                break;
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(top)
}

/// Collect an m:borderBox element.
fn collect_borderbox(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut body = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => body = collect_children(reader, b"m:e", budget)?,
                    b"m:borderBoxPr" => {
                        // Consumes its own `</m:borderBoxPr>`; refund the enter above.
                        skip_to_end(reader, b"m:borderBoxPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:borderBox" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::BorderBox { body })
}

/// Collect an m:m (matrix) element.
fn collect_matrix(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut rows: Vec<Vec<Vec<MathNode>>> = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:mr" => {
                        rows.push(collect_matrix_row(reader, budget)?);
                    }
                    b"m:mPr" => {
                        // Consumes its own `</m:mPr>`; refund the enter above.
                        skip_to_end(reader, b"m:mPr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:m" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::Matrix { rows })
}

/// Collect a matrix row (m:mr) — returns cells.
fn collect_matrix_row(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<Vec<Vec<MathNode>>, SecurityError> {
    let mut cells: Vec<Vec<MathNode>> = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"m:e" => {
                budget.enter()?;
                cells.push(collect_children(reader, b"m:e", budget)?);
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:mr" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(cells)
}

/// Collect an m:sPre (pre-sub-superscript) element.
fn collect_spre(reader: &mut Reader<&[u8]>, budget: &mut SecurityBudget) -> Result<MathNode, SecurityError> {
    let mut base = Vec::new();
    let mut sub = Vec::new();
    let mut sup = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                match e.name().as_ref() as &[u8] {
                    b"m:e" => base = collect_children(reader, b"m:e", budget)?,
                    b"m:sub" => sub = collect_children(reader, b"m:sub", budget)?,
                    b"m:sup" => sup = collect_children(reader, b"m:sup", budget)?,
                    b"m:sPrePr" => {
                        // Consumes its own `</m:sPrePr>`; refund the enter above.
                        skip_to_end(reader, b"m:sPrePr");
                        budget.leave();
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == b"m:sPre" {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(MathNode::SPre { base, sub, sup })
}

/// Collect body of a generic element (skip its *Pr, gather m:e children).
fn collect_element_body(
    reader: &mut Reader<&[u8]>,
    end_tag: &[u8],
    budget: &mut SecurityBudget,
) -> Result<Vec<MathNode>, SecurityError> {
    let mut children = Vec::new();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                budget.enter()?;
                let tag = (e.name().as_ref() as &[u8]).to_vec();
                if tag.ends_with(b"Pr") {
                    // Consumes its own matching end tag; refund the enter above.
                    skip_to_end(reader, &tag);
                    budget.leave();
                } else if tag == b"m:e" {
                    children.extend(collect_children(reader, b"m:e", budget)?);
                } else {
                    // Same as the `*Pr` branch: consumes its own matching end tag.
                    skip_to_end(reader, &tag);
                    budget.leave();
                }
            }
            Ok(Event::End(ref e)) => {
                budget.leave();
                if e.name().as_ref() as &[u8] == end_tag {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }

    Ok(children)
}

/// Get the `m:val` attribute value from a start/empty element.
fn get_m_val(e: &quick_xml::events::BytesStart) -> Option<String> {
    for attr in e.attributes().flatten() {
        let key = attr.key.as_ref();
        if key == b"m:val" || key == b"val" {
            return std::str::from_utf8(&attr.value).ok().map(|s| s.to_string());
        }
    }
    None
}

/// Skip forward until the matching end tag is consumed.
fn skip_to_end(reader: &mut Reader<&[u8]>, tag: &[u8]) {
    let mut depth = 1u32;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == tag => {
                depth += 1;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() as &[u8] == tag => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// Render a slice of MathNodes to LaTeX.
fn render_nodes(nodes: &[MathNode]) -> String {
    let mut out = String::new();
    for node in nodes {
        render_node(node, &mut out);
    }
    out
}

/// Render a single MathNode to LaTeX, appending to `out`.
fn render_node(node: &MathNode, out: &mut String) {
    match node {
        MathNode::Run(text) => {
            render_run_text(text, out);
        }
        MathNode::SSup { base, sup } => {
            render_group(base, out);
            out.push_str("^{");
            out.push_str(&render_nodes(sup));
            out.push('}');
        }
        MathNode::SSub { base, sub } => {
            render_group(base, out);
            out.push_str("_{");
            out.push_str(&render_nodes(sub));
            out.push('}');
        }
        MathNode::SSubSup { base, sub, sup } => {
            render_group(base, out);
            out.push_str("_{");
            out.push_str(&render_nodes(sub));
            out.push_str("}^{");
            out.push_str(&render_nodes(sup));
            out.push('}');
        }
        MathNode::Frac { num, den, frac_type } => match frac_type {
            FracType::Bar => {
                out.push_str("\\frac{");
                out.push_str(&render_nodes(num));
                out.push_str("}{");
                out.push_str(&render_nodes(den));
                out.push('}');
            }
            FracType::NoBar => {
                out.push_str("\\binom{");
                out.push_str(&render_nodes(num));
                out.push_str("}{");
                out.push_str(&render_nodes(den));
                out.push('}');
            }
            FracType::Linear | FracType::Skewed => {
                let num_s = render_nodes(num);
                let den_s = render_nodes(den);
                if num_s.len() > 1 {
                    out.push('{');
                    out.push_str(&num_s);
                    out.push('}');
                } else {
                    out.push_str(&num_s);
                }
                out.push('/');
                if den_s.len() > 1 {
                    out.push('{');
                    out.push_str(&den_s);
                    out.push('}');
                } else {
                    out.push_str(&den_s);
                }
            }
        },
        MathNode::Rad { deg, body, deg_hide } => {
            out.push_str("\\sqrt");
            if !*deg_hide && !deg.is_empty() {
                let deg_s = render_nodes(deg);
                if !deg_s.is_empty() {
                    out.push('[');
                    out.push_str(&deg_s);
                    out.push(']');
                }
            }
            out.push('{');
            out.push_str(&render_nodes(body));
            out.push('}');
        }
        MathNode::Nary {
            chr,
            sub,
            sup,
            body,
            sub_hide,
            sup_hide,
        } => {
            out.push_str(&nary_chr_to_latex(chr));
            if !*sub_hide && !sub.is_empty() {
                out.push_str("_{");
                out.push_str(&render_nodes(sub));
                out.push('}');
            }
            if !*sup_hide && !sup.is_empty() {
                out.push_str("^{");
                out.push_str(&render_nodes(sup));
                out.push('}');
            }
            if !body.is_empty() {
                out.push('{');
                out.push_str(&render_nodes(body));
                out.push('}');
            }
        }
        MathNode::Delim {
            begin_chr,
            end_chr,
            sep_chr,
            elements,
        } => {
            out.push_str("\\left");
            out.push_str(&delim_chr_to_latex(begin_chr));
            for (i, elem) in elements.iter().enumerate() {
                if i > 0 {
                    out.push_str(&delim_sep_to_latex(sep_chr));
                }
                out.push_str(&render_nodes(elem));
            }
            out.push_str("\\right");
            out.push_str(&delim_chr_to_latex(end_chr));
        }
        MathNode::Func { name, body } => {
            let func_name = render_nodes(name);
            let latex_func = match func_name.trim() {
                "sin" => "\\sin",
                "cos" => "\\cos",
                "tan" => "\\tan",
                "cot" => "\\cot",
                "sec" => "\\sec",
                "csc" => "\\csc",
                "log" => "\\log",
                "ln" => "\\ln",
                "exp" => "\\exp",
                "lim" => "\\lim",
                "max" => "\\max",
                "min" => "\\min",
                "sup" => "\\sup",
                "inf" => "\\inf",
                "det" => "\\det",
                "gcd" => "\\gcd",
                "deg" => "\\deg",
                "dim" => "\\dim",
                "hom" => "\\hom",
                "ker" => "\\ker",
                "arg" => "\\arg",
                "sinh" => "\\sinh",
                "cosh" => "\\cosh",
                "tanh" => "\\tanh",
                _ => "",
            };
            if !latex_func.is_empty() {
                out.push_str(latex_func);
            } else {
                out.push_str("\\mathrm{");
                out.push_str(&func_name);
                out.push('}');
            }
            out.push('{');
            out.push_str(&render_nodes(body));
            out.push('}');
        }
        MathNode::Acc { chr, body } => {
            out.push_str(&accent_chr_to_latex(chr));
            out.push('{');
            out.push_str(&render_nodes(body));
            out.push('}');
        }
        MathNode::EqArr { rows } => {
            out.push_str("\\begin{aligned}");
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(" \\\\ ");
                }
                out.push_str(&render_nodes(row));
            }
            out.push_str("\\end{aligned}");
        }
        MathNode::LimLow { body, lim } => {
            out.push_str("\\underset{");
            out.push_str(&render_nodes(lim));
            out.push_str("}{");
            out.push_str(&render_nodes(body));
            out.push('}');
        }
        MathNode::LimUpp { body, lim } => {
            out.push_str("\\overset{");
            out.push_str(&render_nodes(lim));
            out.push_str("}{");
            out.push_str(&render_nodes(body));
            out.push('}');
        }
        MathNode::Bar { body, top } => {
            if *top {
                out.push_str("\\overline{");
            } else {
                out.push_str("\\underline{");
            }
            out.push_str(&render_nodes(body));
            out.push('}');
        }
        MathNode::BorderBox { body } => {
            out.push_str("\\boxed{");
            out.push_str(&render_nodes(body));
            out.push('}');
        }
        MathNode::Matrix { rows } => {
            out.push_str("\\begin{matrix}");
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    out.push_str(" \\\\ ");
                }
                for (j, cell) in row.iter().enumerate() {
                    if j > 0 {
                        out.push_str(" & ");
                    }
                    out.push_str(&render_nodes(cell));
                }
            }
            out.push_str("\\end{matrix}");
        }
        MathNode::Group { children } => {
            out.push_str(&render_nodes(children));
        }
        MathNode::SPre { base, sub, sup } => {
            out.push_str("{}_{");
            out.push_str(&render_nodes(sub));
            out.push_str("}^{");
            out.push_str(&render_nodes(sup));
            out.push('}');
            render_group(base, out);
        }
    }
}

/// Render base nodes, wrapping in braces if needed for subscript/superscript.
fn render_group(nodes: &[MathNode], out: &mut String) {
    let rendered = render_nodes(nodes);
    let needs_braces = rendered.chars().count() > 1 && !rendered.starts_with('\\') && !rendered.starts_with('{');
    if needs_braces {
        out.push('{');
        out.push_str(&rendered);
        out.push('}');
    } else {
        out.push_str(&rendered);
    }
}

/// Map n-ary character to LaTeX command.
fn nary_chr_to_latex(chr: &str) -> String {
    if let Some(ch) = chr.chars().next() {
        match ch {
            '\u{2211}' => return "\\sum".to_string(),
            '\u{220F}' => return "\\prod".to_string(),
            '\u{2210}' => return "\\coprod".to_string(),
            '\u{222B}' => return "\\int".to_string(),
            '\u{222C}' => return "\\iint".to_string(),
            '\u{222D}' => return "\\iiint".to_string(),
            '\u{222E}' => return "\\oint".to_string(),
            '\u{22C0}' => return "\\bigwedge".to_string(),
            '\u{22C1}' => return "\\bigvee".to_string(),
            '\u{22C2}' => return "\\bigcap".to_string(),
            '\u{22C3}' => return "\\bigcup".to_string(),
            _ => {}
        }
    }
    chr.to_string()
}

/// Map delimiter character to LaTeX.
fn delim_chr_to_latex(chr: &str) -> String {
    match chr {
        "(" | ")" | "[" | "]" => chr.to_string(),
        "{" => "\\{".to_string(),
        "}" => "\\}".to_string(),
        "|" => "|".to_string(),
        "\u{2016}" => "\\|".to_string(),
        "\u{2329}" | "\u{27E8}" => "\\langle".to_string(),
        "\u{232A}" | "\u{27E9}" => "\\rangle".to_string(),
        "\u{230A}" => "\\lfloor".to_string(),
        "\u{230B}" => "\\rfloor".to_string(),
        "\u{2308}" => "\\lceil".to_string(),
        "\u{2309}" => "\\rceil".to_string(),
        "" => ".".to_string(),
        _ => chr.to_string(),
    }
}

/// Map delimiter separator character to LaTeX.
fn delim_sep_to_latex(sep: &str) -> String {
    match sep {
        "|" => " \\mid ".to_string(),
        _ => sep.to_string(),
    }
}

/// Map accent character to LaTeX command.
fn accent_chr_to_latex(chr: &str) -> String {
    if let Some(ch) = chr.chars().next() {
        match ch {
            '\u{0302}' | '^' => return "\\hat".to_string(),
            '\u{0303}' | '~' => return "\\tilde".to_string(),
            '\u{0304}' | '\u{0305}' => return "\\bar".to_string(),
            '\u{20D7}' | '\u{2192}' => return "\\vec".to_string(),
            '\u{0307}' => return "\\dot".to_string(),
            '\u{0308}' => return "\\ddot".to_string(),
            '\u{030C}' => return "\\check".to_string(),
            '\u{0306}' => return "\\breve".to_string(),
            '\u{0301}' => return "\\acute".to_string(),
            '\u{0300}' => return "\\grave".to_string(),
            _ => {}
        }
    }
    "\\hat".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse an OMML XML fragment and return rendered LaTeX.
    fn omml_to_latex(xml: &str) -> String {
        let wrapped = format!("<m:oMath>{}</m:oMath>", xml);
        let mut reader = Reader::from_str(&wrapped);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"m:oMath" => break,
                Ok(Event::Eof) => return String::new(),
                _ => {}
            }
            buf.clear();
        }
        let mut budget = SecurityBudget::with_defaults();
        collect_and_convert_omath(&mut reader, &mut budget).unwrap_or_default()
    }

    /// Helper (GH#1395): parse an OMML fragment through `collect_and_convert_omath`
    /// using a caller-supplied budget, so the caller can inspect the budget's
    /// residual depth state after conversion completes.
    fn omml_with_budget(xml: &str, budget: &mut SecurityBudget) {
        let wrapped = format!("<m:oMath>{}</m:oMath>", xml);
        let mut reader = Reader::from_str(&wrapped);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"m:oMath" => break,
                Ok(Event::Eof) => panic!("unexpected EOF before <m:oMath>"),
                _ => {}
            }
            buf.clear();
        }
        collect_and_convert_omath(&mut reader, budget).expect("conversion ok");
    }

    /// Test-only probe (GH#1395): count how many more `budget.enter()` calls succeed
    /// before the depth cap trips. A freshly-built budget accepts exactly `max_depth`
    /// more entries; if the OMML conversion leaked N depth levels, only
    /// `max_depth - N` succeed. This makes the depth counter observable through
    /// `SecurityBudget`'s existing public API without touching `extractors/security.rs`.
    fn probe_remaining_depth(budget: &mut SecurityBudget, max_depth: usize) -> usize {
        let mut successes = 0usize;
        for _ in 0..=max_depth {
            if budget.enter().is_ok() {
                successes += 1;
            } else {
                break;
            }
        }
        successes
    }

    fn budget_with_max_depth(max_depth: usize) -> SecurityBudget {
        let limits = crate::extractors::security::SecurityLimits {
            max_nesting_depth: max_depth,
            max_xml_depth: max_depth,
            ..Default::default()
        };
        SecurityBudget::from_limits(&limits)
    }

    /// GH#1395: `m:rPr` is skipped via `skip_to_end`, which reads through its own
    /// `</m:rPr>` directly — the outer loop's `Event::End` arm never sees it, so
    /// the `budget.enter()` made for `m:rPr`'s start tag must be refunded at the
    /// call site or every run with formatting properties leaks one depth level.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_run_with_rpr() {
        let mut budget = budget_with_max_depth(64);
        omml_with_budget(
            r#"<m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>x</m:t></m:r>"#,
            &mut budget,
        );
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "m:rPr must not leak a depth level"
        );
    }

    /// GH#1395: same `skip_to_end` shape as `m:rPr`, but on a `*Pr` sibling that
    /// hangs off a different element (`m:sSup`).
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_ssup_with_ssuppr() {
        let mut budget = budget_with_max_depth(64);
        omml_with_budget(
            r#"<m:sSup>
                <m:sSupPr><m:ctrlPr/></m:sSupPr>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
                <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
            </m:sSup>"#,
            &mut budget,
        );
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "m:sSupPr must not leak a depth level"
        );
    }

    /// GH#1395: `m:fPr` is handled by `collect_frac_pr`, a dedicated recursive
    /// parser (not `skip_to_end`) that also reads through its own `</m:fPr>`
    /// without calling `budget.leave()` — same leak shape, different mechanism.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_frac_with_fpr() {
        let mut budget = budget_with_max_depth(64);
        omml_with_budget(
            r#"<m:f>
                <m:fPr><m:type m:val="noBar"/></m:fPr>
                <m:num><m:r><m:t>n</m:t></m:r></m:num>
                <m:den><m:r><m:t>k</m:t></m:r></m:den>
            </m:f>"#,
            &mut budget,
        );
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "m:fPr must not leak a depth level"
        );
    }

    /// GH#1395: an unrecognized OMML tag falls into `collect_children`'s `_` arm,
    /// which also delegates to `skip_to_end` and must refund the same way.
    #[test]
    fn should_reset_depth_counter_to_zero_after_parsing_unrecognized_tag() {
        let mut budget = budget_with_max_depth(64);
        omml_with_budget(
            r#"<m:groupChr><m:e><m:r><m:t>x</m:t></m:r></m:e></m:groupChr>"#,
            &mut budget,
        );
        assert_eq!(
            probe_remaining_depth(&mut budget, 64),
            64,
            "an unrecognized OMML tag (skip_to_end fallback) must not leak a depth level"
        );
    }

    #[test]
    fn test_run_plain_text() {
        let latex = omml_to_latex(r#"<m:r><m:t>hello</m:t></m:r>"#);
        assert_eq!(latex, "hello");
    }

    #[test]
    fn test_run_unicode_pi() {
        let latex = omml_to_latex("<m:r><m:t>\u{03C0}</m:t></m:r>");
        assert_eq!(latex, "\\pi ");
    }

    #[test]
    fn test_ssup() {
        let latex = omml_to_latex(
            r#"<m:sSup>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
                <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
            </m:sSup>"#,
        );
        assert_eq!(latex, "x^{2}");
    }

    #[test]
    fn test_ssub() {
        let latex = omml_to_latex(
            r#"<m:sSub>
                <m:e><m:r><m:t>a</m:t></m:r></m:e>
                <m:sub><m:r><m:t>n</m:t></m:r></m:sub>
            </m:sSub>"#,
        );
        assert_eq!(latex, "a_{n}");
    }

    #[test]
    fn test_ssubsup() {
        let latex = omml_to_latex(
            r#"<m:sSubSup>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
                <m:sub><m:r><m:t>i</m:t></m:r></m:sub>
                <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
            </m:sSubSup>"#,
        );
        assert_eq!(latex, "x_{i}^{2}");
    }

    #[test]
    fn test_frac_bar() {
        let latex = omml_to_latex(
            r#"<m:f>
                <m:num><m:r><m:t>a</m:t></m:r></m:num>
                <m:den><m:r><m:t>b</m:t></m:r></m:den>
            </m:f>"#,
        );
        assert_eq!(latex, "\\frac{a}{b}");
    }

    #[test]
    fn test_frac_nobar() {
        let latex = omml_to_latex(
            r#"<m:f>
                <m:fPr><m:type m:val="noBar"/></m:fPr>
                <m:num><m:r><m:t>n</m:t></m:r></m:num>
                <m:den><m:r><m:t>k</m:t></m:r></m:den>
            </m:f>"#,
        );
        assert_eq!(latex, "\\binom{n}{k}");
    }

    #[test]
    fn test_frac_lin() {
        let latex = omml_to_latex(
            r#"<m:f>
                <m:fPr><m:type m:val="lin"/></m:fPr>
                <m:num><m:r><m:t>a</m:t></m:r></m:num>
                <m:den><m:r><m:t>b</m:t></m:r></m:den>
            </m:f>"#,
        );
        assert_eq!(latex, "a/b");
    }

    #[test]
    fn test_rad_simple() {
        let latex = omml_to_latex(
            r#"<m:rad>
                <m:radPr><m:degHide m:val="1"/></m:radPr>
                <m:deg/>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:rad>"#,
        );
        assert_eq!(latex, "\\sqrt{x}");
    }

    #[test]
    fn test_rad_with_degree() {
        let latex = omml_to_latex(
            r#"<m:rad>
                <m:radPr><m:degHide m:val="0"/></m:radPr>
                <m:deg><m:r><m:t>3</m:t></m:r></m:deg>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:rad>"#,
        );
        assert_eq!(latex, "\\sqrt[3]{x}");
    }

    #[test]
    fn test_nary_sum() {
        let latex = omml_to_latex(
            r#"<m:nary>
                <m:naryPr><m:chr m:val="∑"/></m:naryPr>
                <m:sub><m:r><m:t>i=1</m:t></m:r></m:sub>
                <m:sup><m:r><m:t>n</m:t></m:r></m:sup>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:nary>"#,
        );
        assert_eq!(latex, "\\sum_{i=1}^{n}{x}");
    }

    #[test]
    fn test_delim_parens() {
        let latex = omml_to_latex(
            r#"<m:d>
                <m:e><m:r><m:t>x+y</m:t></m:r></m:e>
            </m:d>"#,
        );
        assert_eq!(latex, "\\left(x+y\\right)");
    }

    #[test]
    fn test_delim_brackets() {
        let latex = omml_to_latex(
            r#"<m:d>
                <m:dPr><m:begChr m:val="["/><m:endChr m:val="]"/></m:dPr>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:d>"#,
        );
        assert_eq!(latex, "\\left[x\\right]");
    }

    #[test]
    fn test_acc_hat() {
        let latex = omml_to_latex(
            r#"<m:acc>
                <m:accPr><m:chr m:val="̂"/></m:accPr>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:acc>"#,
        );
        assert_eq!(latex, "\\hat{x}");
    }

    #[test]
    fn test_bar_overline() {
        let latex = omml_to_latex(
            r#"<m:bar>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:bar>"#,
        );
        assert_eq!(latex, "\\overline{x}");
    }

    #[test]
    fn test_bar_underline() {
        let latex = omml_to_latex(
            r#"<m:bar>
                <m:barPr><m:pos m:val="bot"/></m:barPr>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:bar>"#,
        );
        assert_eq!(latex, "\\underline{x}");
    }

    #[test]
    fn test_borderbox() {
        let latex = omml_to_latex(
            r#"<m:borderBox>
                <m:e><m:r><m:t>E=mc</m:t></m:r></m:e>
            </m:borderBox>"#,
        );
        assert_eq!(latex, "\\boxed{E=mc}");
    }

    #[test]
    fn test_matrix() {
        let latex = omml_to_latex(
            r#"<m:m>
                <m:mr>
                    <m:e><m:r><m:t>a</m:t></m:r></m:e>
                    <m:e><m:r><m:t>b</m:t></m:r></m:e>
                </m:mr>
                <m:mr>
                    <m:e><m:r><m:t>c</m:t></m:r></m:e>
                    <m:e><m:r><m:t>d</m:t></m:r></m:e>
                </m:mr>
            </m:m>"#,
        );
        assert_eq!(latex, "\\begin{matrix}a & b \\\\ c & d\\end{matrix}");
    }

    #[test]
    fn test_eqarr() {
        let latex = omml_to_latex(
            r#"<m:eqArr>
                <m:e><m:r><m:t>x=1</m:t></m:r></m:e>
                <m:e><m:r><m:t>y=2</m:t></m:r></m:e>
            </m:eqArr>"#,
        );
        assert_eq!(latex, "\\begin{aligned}x=1 \\\\ y=2\\end{aligned}");
    }

    #[test]
    fn test_func() {
        let latex = omml_to_latex(
            r#"<m:func>
                <m:fName><m:r><m:t>sin</m:t></m:r></m:fName>
                <m:e><m:r><m:t>x</m:t></m:r></m:e>
            </m:func>"#,
        );
        assert_eq!(latex, "\\sin{x}");
    }

    #[test]
    fn test_limlow() {
        let latex = omml_to_latex(
            r#"<m:limLow>
                <m:e><m:r><m:t>lim</m:t></m:r></m:e>
                <m:lim><m:r><m:t>n→∞</m:t></m:r></m:lim>
            </m:limLow>"#,
        );
        assert_eq!(latex, "\\underset{n\\rightarrow \\infty }{lim}");
    }

    #[test]
    fn test_nested_quadratic_formula() {
        let latex = omml_to_latex(
            r#"<m:r><m:t>x=</m:t></m:r>
            <m:f>
                <m:num>
                    <m:r><m:t>-b</m:t></m:r>
                    <m:r><m:t>±</m:t></m:r>
                    <m:rad>
                        <m:radPr><m:degHide m:val="1"/></m:radPr>
                        <m:deg/>
                        <m:e>
                            <m:sSup>
                                <m:e><m:r><m:t>b</m:t></m:r></m:e>
                                <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
                            </m:sSup>
                            <m:r><m:t>-4ac</m:t></m:r>
                        </m:e>
                    </m:rad>
                </m:num>
                <m:den>
                    <m:r><m:t>2a</m:t></m:r>
                </m:den>
            </m:f>"#,
        );
        assert_eq!(latex, "x=\\frac{-b\\pm \\sqrt{b^{2}-4ac}}{2a}");
    }

    #[test]
    fn test_omath_para_display() {
        let xml = r#"<m:oMathPara><m:oMath><m:r><m:t>E=mc</m:t></m:r><m:sSup><m:e><m:r><m:t/></m:r></m:e><m:sup><m:r><m:t>2</m:t></m:r></m:sup></m:sSup></m:oMath></m:oMathPara>"#;
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        loop {
            match reader.read_event_into(&mut buf) {
                Ok(Event::Start(ref e)) if e.name().as_ref() as &[u8] == b"m:oMathPara" => break,
                Ok(Event::Eof) => panic!("unexpected EOF"),
                _ => {}
            }
            buf.clear();
        }
        let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
        let latex = collect_and_convert_omath_para(&mut reader, &mut budget).expect("conversion ok");
        assert!(latex.contains("E=mc"));
        assert!(latex.contains("^{2}"));
    }

    #[test]
    fn test_run_with_rpr() {
        let latex = omml_to_latex(r#"<m:r><m:rPr><m:sty m:val="p"/></m:rPr><m:t>x</m:t></m:r>"#);
        assert_eq!(latex, "x");
    }

    #[test]
    fn test_nary_integral_default() {
        let latex = omml_to_latex(
            r#"<m:nary>
                <m:naryPr/>
                <m:sub><m:r><m:t>0</m:t></m:r></m:sub>
                <m:sup><m:r><m:t>1</m:t></m:r></m:sup>
                <m:e><m:r><m:t>f(x)dx</m:t></m:r></m:e>
            </m:nary>"#,
        );
        assert_eq!(latex, "\\int_{0}^{1}{f(x)dx}");
    }

    #[test]
    fn test_spre() {
        let latex = omml_to_latex(
            r#"<m:sPre>
                <m:sub><m:r><m:t>2</m:t></m:r></m:sub>
                <m:sup><m:r><m:t>3</m:t></m:r></m:sup>
                <m:e><m:r><m:t>X</m:t></m:r></m:e>
            </m:sPre>"#,
        );
        assert_eq!(latex, "{}_{2}^{3}X");
    }

    #[test]
    fn test_delim_multiple_elements() {
        let latex = omml_to_latex(
            r#"<m:d>
                <m:e><m:r><m:t>a</m:t></m:r></m:e>
                <m:e><m:r><m:t>b</m:t></m:r></m:e>
            </m:d>"#,
        );
        assert_eq!(latex, "\\left(a \\mid b\\right)");
    }
}
