//! XML parsing for PPTX slide content.
//!
//! This module handles parsing slide XML, extracting text, tables, lists, images,
//! and relationships from PowerPoint presentations.

use ahash::AHashMap;
use roxmltree::{Document, Node};

use crate::error::{Result, XbergError};
use crate::text::utf8_validation;

use super::elements::{
    ChartReference, DiagramReference, ElementPosition, Formatting, ImageReference, ListElement, ListItem,
    ParsedContent, Run, SlideElement, TableCell, TableElement, TableRow, TextElement,
};

use crate::extraction::ooxml_constants::{DRAWINGML_NAMESPACE, PRESENTATIONML_NAMESPACE, RELATIONSHIPS_NAMESPACE};

/// Markup-compatibility namespace (`mc:AlternateContent`/`mc:Choice`/`mc:Fallback`).
///
/// PowerPoint wraps most non-baseline shape content (extension shapes,
/// connectors carrying newer geometry, and OMML math) in `mc:AlternateContent`
/// so older readers can fall back to a simpler representation. A namespace
/// filter that only accepts PresentationML elements silently drops the whole
/// subtree — see #79.
const MARKUP_COMPATIBILITY_NAMESPACE: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";

/// OMML (Office Math Markup Language) namespace for `m:oMath`/`m:oMathPara`.
const MATH_NAMESPACE: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

/// DrawingML chart namespace for `p:graphicFrame` chart payloads.
const CHART_NAMESPACE: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

/// DrawingML diagram (SmartArt) namespace for `p:graphicFrame` diagram payloads.
const DIAGRAM_NAMESPACE: &str = "http://schemas.openxmlformats.org/drawingml/2006/diagram";

/// A `p:graphicFrame`'s parsed payload: a table, a chart reference, or a
/// SmartArt/diagram reference. Chart and diagram text lives in a separate ZIP
/// part resolved later against the slide's relationships (see
/// `container::resolve_graphic_frame_text`).
enum GraphicFrameContent {
    Table(TableElement),
    Chart(ChartReference),
    SmartArt(DiagramReference),
}

pub(super) fn parse_slide_xml(xml_data: &[u8]) -> Result<Vec<SlideElement>> {
    let xml_str = utf8_validation::from_utf8(xml_data)
        .map_err(|_| XbergError::parsing("Invalid UTF-8 in slide XML".to_string()))?;

    let doc = Document::parse(xml_str).map_err(|e| XbergError::parsing(format!("Failed to parse slide XML: {}", e)))?;

    let root = doc.root_element();
    let ns = root.tag_name().namespace();

    let c_sld = root
        .descendants()
        .find(|n| n.tag_name().name() == "cSld" && n.tag_name().namespace() == ns)
        .ok_or_else(|| XbergError::parsing("No <p:cSld> tag found".to_string()))?;

    let sp_tree = c_sld
        .children()
        .find(|n| n.tag_name().name() == "spTree" && n.tag_name().namespace() == ns)
        .ok_or_else(|| XbergError::parsing("No <p:spTree> tag found".to_string()))?;

    let mut elements = Vec::new();
    for child_node in sp_tree.children().filter(|n| n.is_element()) {
        elements.extend(parse_group(&child_node, xml_str)?);
    }

    Ok(elements)
}

/// Parse a shape-tree node into zero or more `SlideElement`s.
///
/// `xml_str` is the full original slide document text, needed to slice out
/// raw OMML XML by byte range for the shared OMML-to-LaTeX converter (see
/// `omml_node_to_run`).
fn parse_group(node: &Node, xml_str: &str) -> Result<Vec<SlideElement>> {
    let tag_name = node.tag_name().name();
    let namespace = node.tag_name().namespace().unwrap_or("");

    // mc:AlternateContent can wrap an entire shape (extension shapes, newer
    // connector geometry). Recurse into mc:Choice (preferred) or mc:Fallback
    // rather than dropping the subtree — see #79.
    if namespace == MARKUP_COMPATIBILITY_NAMESPACE && tag_name == "AlternateContent" {
        return parse_alternate_content_shapes(node, xml_str);
    }

    let mut elements = Vec::new();

    if namespace != PRESENTATIONML_NAMESPACE {
        return Ok(elements);
    }

    let position = extract_position(node);

    match tag_name {
        "sp" | "cxnSp" => {
            if let Some(content) = parse_sp(node, xml_str)? {
                match content {
                    ParsedContent::Text(text) => elements.push(SlideElement::Text(text, position)),
                    ParsedContent::List(list) => elements.push(SlideElement::List(list, position)),
                }
            }
        }
        "graphicFrame" => {
            if let Some(content) = parse_graphic_frame(node)? {
                match content {
                    GraphicFrameContent::Table(table) => elements.push(SlideElement::Table(table, position)),
                    GraphicFrameContent::Chart(chart) => elements.push(SlideElement::Chart(chart, position)),
                    GraphicFrameContent::SmartArt(diagram) => elements.push(SlideElement::SmartArt(diagram, position)),
                }
            }
        }
        "pic" => match parse_pic(node) {
            Ok(image_reference) => elements.push(SlideElement::Image(image_reference, position)),
            Err(e) => tracing::warn!("Skipping broken image in slide: {}", e),
        },
        "grpSp" => {
            for child in node.children().filter(|n| n.is_element()) {
                elements.extend(parse_group(&child, xml_str)?);
            }
        }
        _ => elements.push(SlideElement::Unknown),
    }

    Ok(elements)
}

/// Resolve an `mc:AlternateContent` shape wrapper: prefer `mc:Choice` content
/// (the modern representation, e.g. an extension shape or connector), and
/// fall back to `mc:Fallback` only if `mc:Choice` yielded nothing.
fn parse_alternate_content_shapes(node: &Node, xml_str: &str) -> Result<Vec<SlideElement>> {
    let choice = node.children().find(|n| {
        n.is_element()
            && n.tag_name().namespace() == Some(MARKUP_COMPATIBILITY_NAMESPACE)
            && n.tag_name().name() == "Choice"
    });
    let fallback = node.children().find(|n| {
        n.is_element()
            && n.tag_name().namespace() == Some(MARKUP_COMPATIBILITY_NAMESPACE)
            && n.tag_name().name() == "Fallback"
    });

    if let Some(choice_node) = choice {
        let mut elements = Vec::new();
        for child in choice_node.children().filter(|n| n.is_element()) {
            elements.extend(parse_group(&child, xml_str)?);
        }
        if !elements.is_empty() {
            return Ok(elements);
        }
    }

    if let Some(fallback_node) = fallback {
        let mut elements = Vec::new();
        for child in fallback_node.children().filter(|n| n.is_element()) {
            elements.extend(parse_group(&child, xml_str)?);
        }
        return Ok(elements);
    }

    Ok(Vec::new())
}

/// Check whether a shape node contains a title placeholder.
///
/// OOXML placeholder types for titles:
/// - `type="title"` (general title, idx 0 in most slide layouts)
/// - `type="ctrTitle"` (centered title, used on title slides)
fn is_title_placeholder(sp_node: &Node) -> bool {
    let nv_sp_pr = sp_node
        .children()
        .find(|n| n.tag_name().namespace() == Some(PRESENTATIONML_NAMESPACE) && n.tag_name().name().starts_with("nv"));
    if let Some(nv_sp_pr) = nv_sp_pr {
        let nv_pr = nv_sp_pr
            .children()
            .find(|n| n.tag_name().name() == "nvPr" && n.tag_name().namespace() == Some(PRESENTATIONML_NAMESPACE));
        if let Some(nv_pr) = nv_pr {
            let ph = nv_pr
                .children()
                .find(|n| n.tag_name().name() == "ph" && n.tag_name().namespace() == Some(PRESENTATIONML_NAMESPACE));
            if let Some(ph) = ph {
                return matches!(ph.attribute("type"), Some("title") | Some("ctrTitle"));
            }
        }
    }
    false
}

fn parse_sp(sp_node: &Node, xml_str: &str) -> Result<Option<ParsedContent>> {
    let tx_body_node = match sp_node
        .children()
        .find(|n| n.tag_name().name() == "txBody" && n.tag_name().namespace() == Some(PRESENTATIONML_NAMESPACE))
    {
        Some(node) => node,
        None => return Ok(None),
    };

    let is_title = is_title_placeholder(sp_node);

    let is_list = !is_title
        && tx_body_node.descendants().any(|n| {
            n.is_element()
                && n.tag_name().name() == "pPr"
                && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
                && (n.attribute("lvl").is_some()
                    || n.children().any(|child| {
                        child.is_element()
                            && (child.tag_name().name() == "buAutoNum" || child.tag_name().name() == "buChar")
                    }))
        });

    if is_list {
        Ok(Some(ParsedContent::List(parse_list(&tx_body_node, xml_str)?)))
    } else {
        let mut text_el = parse_text(&tx_body_node, xml_str)?;
        text_el.is_title = is_title;
        Ok(Some(ParsedContent::Text(text_el)))
    }
}

pub(super) fn parse_text(tx_body_node: &Node, xml_str: &str) -> Result<TextElement> {
    let mut runs = Vec::new();

    for p_node in tx_body_node.children().filter(|n| {
        n.is_element() && n.tag_name().name() == "p" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        let mut paragraph_runs = parse_paragraph(&p_node, true, xml_str)?;
        runs.append(&mut paragraph_runs);
    }

    Ok(TextElement { runs, is_title: false })
}

fn parse_graphic_frame(node: &Node) -> Result<Option<GraphicFrameContent>> {
    let graphic_data_node = node.descendants().find(|n| {
        n.is_element() && n.tag_name().name() == "graphicData" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    });

    let Some(graphic_data) = graphic_data_node else {
        return Ok(None);
    };

    let uri = graphic_data.attribute("uri").unwrap_or("");

    if uri == "http://schemas.openxmlformats.org/drawingml/2006/table" {
        if let Some(tbl_node) = graphic_data.children().find(|n| {
            n.is_element() && n.tag_name().name() == "tbl" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
        }) {
            return Ok(Some(GraphicFrameContent::Table(parse_table(&tbl_node)?)));
        }
        return Ok(None);
    }

    if uri == CHART_NAMESPACE {
        let rel_id = graphic_data
            .children()
            .find(|n| {
                n.is_element() && n.tag_name().name() == "chart" && n.tag_name().namespace() == Some(CHART_NAMESPACE)
            })
            .and_then(|n| {
                n.attribute((RELATIONSHIPS_NAMESPACE, "id"))
                    .or_else(|| n.attribute("r:id"))
                    .map(|s| s.to_string())
            });
        return Ok(rel_id.map(|rel_id| {
            GraphicFrameContent::Chart(ChartReference {
                rel_id,
                resolved_text: None,
            })
        }));
    }

    if uri == DIAGRAM_NAMESPACE {
        let rel_id = graphic_data
            .children()
            .find(|n| {
                n.is_element() && n.tag_name().name() == "relIds" && n.tag_name().namespace() == Some(DIAGRAM_NAMESPACE)
            })
            .and_then(|n| {
                n.attribute((RELATIONSHIPS_NAMESPACE, "dm"))
                    .or_else(|| n.attribute("r:dm"))
                    .map(|s| s.to_string())
            });
        return Ok(rel_id.map(|rel_id| {
            GraphicFrameContent::SmartArt(DiagramReference {
                rel_id,
                resolved_text: None,
            })
        }));
    }

    Ok(None)
}

/// Parse the text content of a chart part (e.g. `ppt/charts/chart1.xml`).
///
/// Recovers the chart title and every cached string/numeric value (`c:v`),
/// which together cover series names, category labels, and data points.
/// Returns `Ok(None)` when the chart carries no recoverable text.
pub(super) fn parse_chart_text(xml_data: &[u8]) -> Result<Option<String>> {
    let xml_str = utf8_validation::from_utf8(xml_data)
        .map_err(|e| XbergError::parsing(format!("Invalid UTF-8 in chart XML: {}", e)))?;
    let doc = Document::parse(xml_str).map_err(|e| XbergError::parsing(format!("Failed to parse chart XML: {}", e)))?;

    let title = doc
        .descendants()
        .find(|n| n.tag_name().namespace() == Some(CHART_NAMESPACE) && n.tag_name().name() == "title")
        .map(|title_node| {
            title_node
                .descendants()
                .filter(|n| n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE) && n.tag_name().name() == "t")
                .filter_map(|n| n.text())
                .collect::<Vec<_>>()
                .join("")
        })
        .filter(|s| !s.trim().is_empty());

    let values: Vec<String> = doc
        .descendants()
        .filter(|n| n.tag_name().namespace() == Some(CHART_NAMESPACE) && n.tag_name().name() == "v")
        .filter_map(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let mut parts = Vec::new();
    if let Some(t) = title {
        parts.push(t);
    }
    if !values.is_empty() {
        parts.push(values.join(", "));
    }

    if parts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parts.join("\n")))
    }
}

/// Parse the text content of a SmartArt/diagram data part (e.g.
/// `ppt/diagrams/data1.xml`). Every diagram node's text lives in a
/// `<dgm:t>` element containing a normal DrawingML paragraph/run body, so
/// collecting all `a:t` descendants recovers every node's text.
/// Returns `Ok(None)` when the diagram carries no recoverable text.
pub(super) fn parse_diagram_text(xml_data: &[u8]) -> Result<Option<String>> {
    let xml_str = utf8_validation::from_utf8(xml_data)
        .map_err(|e| XbergError::parsing(format!("Invalid UTF-8 in diagram XML: {}", e)))?;
    let doc =
        Document::parse(xml_str).map_err(|e| XbergError::parsing(format!("Failed to parse diagram XML: {}", e)))?;

    let texts: Vec<String> = doc
        .descendants()
        .filter(|n| n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE) && n.tag_name().name() == "t")
        .filter_map(|n| n.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if texts.is_empty() {
        Ok(None)
    } else {
        Ok(Some(texts.join("\n")))
    }
}

fn parse_table(tbl_node: &Node) -> Result<TableElement> {
    let mut rows = Vec::new();

    for tr_node in tbl_node.children().filter(|n| {
        n.is_element() && n.tag_name().name() == "tr" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        let row = parse_table_row(&tr_node)?;
        rows.push(row);
    }

    Ok(TableElement { rows })
}

fn parse_table_row(tr_node: &Node) -> Result<TableRow> {
    let mut cells = Vec::new();

    for tc_node in tr_node.children().filter(|n| {
        n.is_element() && n.tag_name().name() == "tc" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        let cell = parse_table_cell(&tc_node)?;
        cells.push(cell);
    }

    Ok(TableRow { cells })
}

fn parse_table_cell(tc_node: &Node) -> Result<TableCell> {
    let mut runs = Vec::new();

    if let Some(tx_body_node) = tc_node.children().find(|n| {
        n.is_element() && n.tag_name().name() == "txBody" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        // Table cells use the `a:txBody` schema directly (no `xml_str` slicing
        // needed for math, since cell text is treated as plain runs); pass an
        // empty document string as there is nothing to slice for OMML here.
        for p_node in tx_body_node.children().filter(|n| {
            n.is_element() && n.tag_name().name() == "p" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
        }) {
            let mut paragraph_runs = parse_paragraph(&p_node, false, "")?;
            runs.append(&mut paragraph_runs);
        }
    }

    Ok(TableCell { runs })
}

fn parse_pic(pic_node: &Node) -> Result<ImageReference> {
    let blip_node = pic_node
        .descendants()
        .find(|n| {
            n.is_element() && n.tag_name().name() == "blip" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
        })
        .ok_or_else(|| XbergError::parsing("Image blip not found".to_string()))?;

    let embed_attr = blip_node
        .attribute((RELATIONSHIPS_NAMESPACE, "embed"))
        .or_else(|| blip_node.attribute("r:embed"))
        .ok_or_else(|| XbergError::parsing("Image embed attribute not found".to_string()))?;

    let description = pic_node
        .descendants()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "cNvPr"
                && n.tag_name().namespace() == Some(PRESENTATIONML_NAMESPACE)
        })
        .or_else(|| {
            pic_node.descendants().find(|n| {
                n.is_element()
                    && n.tag_name().name() == "cNvPr"
                    && n.parent().is_some_and(|p| p.tag_name().name() == "nvPicPr")
            })
        })
        .and_then(|cnv| cnv.attribute("descr"))
        .filter(|d| !d.is_empty())
        .map(|d| d.to_string());

    let image_ref = ImageReference {
        id: embed_attr.to_string(),
        target: String::new(),
        description,
    };

    Ok(image_ref)
}

fn parse_list(tx_body_node: &Node, xml_str: &str) -> Result<ListElement> {
    let mut items = Vec::new();

    for p_node in tx_body_node.children().filter(|n| {
        n.is_element() && n.tag_name().name() == "p" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        let (level, is_ordered, has_bullet) = parse_list_properties(&p_node)?;

        let runs = parse_paragraph(&p_node, true, xml_str)?;

        items.push(ListItem {
            level,
            is_ordered,
            runs,
            has_bullet,
        });
    }

    Ok(ListElement { items })
}

/// Returns (level, is_ordered, has_bullet).
fn parse_list_properties(p_node: &Node) -> Result<(u32, bool, bool)> {
    let mut level = 1;
    let mut is_ordered = false;
    let mut has_bullet = false;

    if let Some(p_pr_node) = p_node.children().find(|n| {
        n.is_element() && n.tag_name().name() == "pPr" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        if let Some(lvl_attr) = p_pr_node.attribute("lvl") {
            level = lvl_attr.parse::<u32>().unwrap_or(0) + 1;
        }

        is_ordered = p_pr_node.children().any(|n| {
            n.is_element()
                && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
                && n.tag_name().name() == "buAutoNum"
        });

        let has_bu_none = p_pr_node.children().any(|n| {
            n.is_element() && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE) && n.tag_name().name() == "buNone"
        });
        has_bullet = !has_bu_none
            && (is_ordered
                || p_pr_node.children().any(|n| {
                    n.is_element()
                        && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
                        && n.tag_name().name() == "buChar"
                })
                || p_pr_node.attribute("lvl").is_some());
    }

    Ok((level, is_ordered, has_bullet))
}

/// Parse a paragraph's inline content into a flat run list, in document order.
///
/// Handles the run types PowerPoint emits as direct `a:p` children:
/// - `a:r` — a normal formatted text run.
/// - `a:fld` — a field run (slide number, date/time); PowerPoint caches the
///   rendered text in a nested `a:t`, so the field is read like a run (#90).
/// - `a:br` — an explicit in-paragraph line break, emitted as a `"\n"` run (#90).
/// - `mc:AlternateContent` — an extension wrapper; `mc:Choice` commonly carries
///   OMML math (`a14:m/m:oMath(Para)`) with `mc:Fallback` holding a plain-text
///   or image substitute for older readers (#47, #79).
/// - `m:oMathPara` / `m:oMath` — math written straight into the paragraph, with
///   no compatibility wrapper. DOCX paragraphs accept the same two shapes.
///
/// `add_new_line` mirrors the pre-existing behavior of appending a trailing
/// `"\n"` after the paragraph's last run (matching a `<a:p>` boundary).
fn parse_paragraph(p_node: &Node, add_new_line: bool, xml_str: &str) -> Result<Vec<Run>> {
    let mut runs: Vec<Run> = Vec::new();

    for child in p_node.children().filter(|n| n.is_element()) {
        let ns = child.tag_name().namespace();
        let name = child.tag_name().name();

        if ns == Some(DRAWINGML_NAMESPACE) && name == "r" {
            runs.push(parse_run(&child)?);
        } else if ns == Some(DRAWINGML_NAMESPACE) && name == "br" {
            runs.push(line_break_run());
        } else if ns == Some(DRAWINGML_NAMESPACE) && name == "fld" {
            runs.push(parse_field(&child));
        } else if ns == Some(MARKUP_COMPATIBILITY_NAMESPACE) && name == "AlternateContent" {
            runs.extend(parse_alternate_content_runs(&child, xml_str));
        } else if ns == Some(MATH_NAMESPACE)
            && (name == "oMathPara" || name == "oMath")
            && let Some(math_run) = omml_node_to_run(&child, xml_str)
        {
            runs.push(math_run);
        }
    }

    if add_new_line {
        match runs.last_mut() {
            Some(last) if last.math_latex.is_none() => last.text.push('\n'),
            Some(_) => runs.push(line_break_run()),
            None => {}
        }
    }

    Ok(runs)
}

fn line_break_run() -> Run {
    Run {
        text: "\n".to_string(),
        formatting: Formatting::default(),
        hyperlink_id: None,
        math_latex: None,
    }
}

/// Parse an `a:fld` field run (slide number, date/time, etc.).
///
/// PowerPoint always caches the rendered field value in a nested `a:t`, so a
/// field is read exactly like a run: formatting from `a:rPr`, text from `a:t`.
fn parse_field(fld_node: &Node) -> Run {
    let mut formatting = Formatting::default();

    if let Some(r_pr_node) = fld_node.children().find(|n| {
        n.is_element() && n.tag_name().name() == "rPr" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        apply_run_properties(&r_pr_node, &mut formatting);
    }

    let text = fld_node
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "t" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE))
        .and_then(|t| t.text())
        .unwrap_or("")
        .to_string();

    Run {
        text,
        formatting,
        hyperlink_id: None,
        math_latex: None,
    }
}

/// Resolve an `mc:AlternateContent` run-level wrapper found inside a paragraph.
///
/// Prefers `mc:Choice` (where OMML math lives), falling back to `mc:Fallback`
/// runs when the choice carries no recognized content.
fn parse_alternate_content_runs(node: &Node, xml_str: &str) -> Vec<Run> {
    let choice = node.children().find(|n| {
        n.is_element()
            && n.tag_name().namespace() == Some(MARKUP_COMPATIBILITY_NAMESPACE)
            && n.tag_name().name() == "Choice"
    });
    let fallback = node.children().find(|n| {
        n.is_element()
            && n.tag_name().namespace() == Some(MARKUP_COMPATIBILITY_NAMESPACE)
            && n.tag_name().name() == "Fallback"
    });

    if let Some(choice_node) = choice {
        if let Some(math_run) = omml_node_to_run(&choice_node, xml_str) {
            return vec![math_run];
        }
        let runs = collect_runs_from_descendants(&choice_node);
        if !runs.is_empty() {
            return runs;
        }
    }

    if let Some(fallback_node) = fallback {
        return collect_runs_from_descendants(&fallback_node);
    }

    Vec::new()
}

/// Collect `a:r`/`a:br`/`a:fld` runs from anywhere within `container`, used
/// for `mc:Choice`/`mc:Fallback` bodies whose structure varies by extension.
fn collect_runs_from_descendants(container: &Node) -> Vec<Run> {
    let mut out = Vec::new();
    for n in container.descendants() {
        if !n.is_element() {
            continue;
        }
        let ns = n.tag_name().namespace();
        let name = n.tag_name().name();
        if ns == Some(DRAWINGML_NAMESPACE) && name == "r" {
            if let Ok(run) = parse_run(&n) {
                out.push(run);
            }
        } else if ns == Some(DRAWINGML_NAMESPACE) && name == "br" {
            out.push(line_break_run());
        } else if ns == Some(DRAWINGML_NAMESPACE) && name == "fld" {
            out.push(parse_field(&n));
        }
    }
    out
}

/// Find the first OMML `m:oMathPara`/`m:oMath` node under `container` and
/// convert it to LaTeX via the shared OMML converter (`extraction::docx::math`),
/// producing a single math `Run`. Returns `None` if no math node is present or
/// conversion fails.
///
/// The converter is a `quick_xml` streaming API positioned just after the
/// start tag, while this module parses with `roxmltree` (a DOM). To bridge
/// the two, the math node's byte range in the original document text
/// (`xml_str`) is sliced out and re-parsed with a fresh `quick_xml::Reader`.
fn omml_node_to_run(container: &Node, xml_str: &str) -> Option<Run> {
    let (is_display, math_node) = if let Some(n) = container
        .descendants()
        .find(|d| d.tag_name().namespace() == Some(MATH_NAMESPACE) && d.tag_name().name() == "oMathPara")
    {
        (true, n)
    } else if let Some(n) = container
        .descendants()
        .find(|d| d.tag_name().namespace() == Some(MATH_NAMESPACE) && d.tag_name().name() == "oMath")
    {
        (false, n)
    } else {
        return None;
    };

    let raw_xml = xml_str.get(math_node.range())?;
    let expected_tag: &[u8] = if is_display { b"m:oMathPara" } else { b"m:oMath" };

    let mut reader = quick_xml::Reader::from_str(raw_xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(ref e)) if e.name().as_ref() == expected_tag => break,
            Ok(quick_xml::events::Event::Eof) => return None,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }

    let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
    let latex = if is_display {
        crate::extraction::docx::math::collect_and_convert_omath_para(&mut reader, &mut budget).ok()?
    } else {
        crate::extraction::docx::math::collect_and_convert_omath(&mut reader, &mut budget).ok()?
    };

    if latex.is_empty() {
        return None;
    }

    Some(Run {
        text: String::new(),
        formatting: Formatting::default(),
        hyperlink_id: None,
        math_latex: Some((latex, is_display)),
    })
}

/// Apply `a:rPr` run-property attributes and `a:hlinkClick` to `formatting`,
/// returning the resolved hyperlink relationship ID if present.
fn apply_run_properties(r_pr_node: &Node, formatting: &mut Formatting) -> Option<String> {
    if let Some(b_attr) = r_pr_node.attribute("b") {
        formatting.bold = b_attr == "1" || b_attr.eq_ignore_ascii_case("true");
    }
    if let Some(i_attr) = r_pr_node.attribute("i") {
        formatting.italic = i_attr == "1" || i_attr.eq_ignore_ascii_case("true");
    }
    if let Some(u_attr) = r_pr_node.attribute("u") {
        formatting.underlined = u_attr != "none";
    }
    if let Some(strike_attr) = r_pr_node.attribute("strike") {
        formatting.strikethrough = matches!(strike_attr, "sngStrike" | "dblStrike");
    }
    if let Some(sz_attr) = r_pr_node.attribute("sz") {
        formatting.font_size = sz_attr.parse::<u32>().ok();
    }
    if let Some(lang_attr) = r_pr_node.attribute("lang") {
        formatting.lang = lang_attr.to_string();
    }

    r_pr_node
        .children()
        .find(|n| {
            n.is_element()
                && n.tag_name().name() == "hlinkClick"
                && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
        })
        .and_then(|hlink_node| {
            hlink_node
                .attribute((RELATIONSHIPS_NAMESPACE, "id"))
                .or_else(|| hlink_node.attribute("r:id"))
                .map(|s| s.to_string())
        })
}

fn parse_run(r_node: &Node) -> Result<Run> {
    let mut text = String::new();
    let mut formatting = Formatting::default();
    let mut hyperlink_id = None;

    if let Some(r_pr_node) = r_node.children().find(|n| {
        n.is_element() && n.tag_name().name() == "rPr" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE)
    }) {
        hyperlink_id = apply_run_properties(&r_pr_node, &mut formatting);
    }

    if let Some(t_node) = r_node
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "t" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE))
        && let Some(t) = t_node.text()
    {
        text.push_str(t);
    }
    Ok(Run {
        text,
        formatting,
        hyperlink_id,
        math_latex: None,
    })
}

pub(super) fn extract_position(node: &Node) -> ElementPosition {
    let default = ElementPosition::default();

    node.descendants()
        .find(|n| n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE) && n.tag_name().name() == "xfrm")
        .and_then(|xfrm| {
            let x = xfrm
                .children()
                .find(|n| n.tag_name().name() == "off" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE))
                .and_then(|off| off.attribute("x")?.parse::<i64>().ok())?;

            let y = xfrm
                .children()
                .find(|n| n.tag_name().name() == "off" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE))
                .and_then(|off| off.attribute("y")?.parse::<i64>().ok())?;

            let (cx, cy) = xfrm
                .children()
                .find(|n| n.tag_name().name() == "ext" && n.tag_name().namespace() == Some(DRAWINGML_NAMESPACE))
                .map(|ext| {
                    let cx = ext.attribute("cx").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                    let cy = ext.attribute("cy").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                    (cx, cy)
                })
                .unwrap_or((0, 0));

            Some(ElementPosition { x, y, cx, cy })
        })
        .unwrap_or(default)
}

/// Parsed relationships from a slide rels file.
pub(super) struct SlideRels {
    pub(super) images: Vec<ImageReference>,
    pub(super) hyperlinks: Vec<super::elements::HyperlinkReference>,
    /// Every relationship ID in the rels file mapped to its target, regardless
    /// of relationship type. Used to resolve chart/SmartArt `graphicData`
    /// references, which are neither images nor hyperlinks.
    pub(super) targets: AHashMap<String, String>,
}

pub(super) fn parse_slide_rels(rels_data: &[u8]) -> Result<SlideRels> {
    let xml_str = utf8_validation::from_utf8(rels_data)
        .map_err(|e| XbergError::parsing(format!("Invalid UTF-8 in rels XML: {}", e)))?;

    let doc = Document::parse(xml_str).map_err(|e| XbergError::parsing(format!("Failed to parse rels XML: {}", e)))?;

    let mut images = Vec::new();
    let mut hyperlinks = Vec::new();
    let mut targets = AHashMap::new();

    for node in doc.descendants() {
        if node.has_tag_name("Relationship")
            && let Some(rel_type) = node.attribute("Type")
            && let (Some(id), Some(target)) = (node.attribute("Id"), node.attribute("Target"))
        {
            targets.insert(id.to_string(), target.to_string());

            if rel_type.contains("image") {
                images.push(ImageReference {
                    id: id.to_string(),
                    target: target.to_string(),
                    description: None,
                });
            } else if rel_type.contains("hyperlink") {
                hyperlinks.push(super::elements::HyperlinkReference {
                    id: id.to_string(),
                    url: target.to_string(),
                });
            }
        }
    }

    Ok(SlideRels {
        images,
        hyperlinks,
        targets,
    })
}

pub(super) fn parse_presentation_rels(rels_data: &[u8]) -> Result<Vec<String>> {
    let xml_str = utf8_validation::from_utf8(rels_data)
        .map_err(|e| XbergError::parsing(format!("Invalid UTF-8 in presentation rels: {}", e)))?;

    let doc = Document::parse(xml_str)
        .map_err(|e| XbergError::parsing(format!("Failed to parse presentation rels: {}", e)))?;

    let mut slide_paths = Vec::new();

    for node in doc.descendants() {
        if node.has_tag_name("Relationship")
            && let Some(rel_type) = node.attribute("Type")
            && rel_type.contains("slide")
            && !rel_type.contains("slideMaster")
            && let Some(target) = node.attribute("Target")
        {
            let normalized_target = target.strip_prefix('/').unwrap_or(target);
            let final_path = if normalized_target.starts_with("ppt/") {
                normalized_target.to_string()
            } else {
                format!("ppt/{}", normalized_target)
            };
            slide_paths.push(final_path);
        }
    }

    slide_paths.sort_by(|a, b| {
        fn slide_num(s: &str) -> u32 {
            s.rsplit('/')
                .next()
                .unwrap_or("")
                .strip_prefix("slide")
                .unwrap_or("")
                .strip_suffix(".xml")
                .unwrap_or("")
                .parse()
                .unwrap_or(u32::MAX)
        }
        slide_num(a).cmp(&slide_num(b))
    });

    Ok(slide_paths)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_slide_paths_sorted_numerically() {
        let mut paths = vec![
            "ppt/slides/slide1.xml".to_string(),
            "ppt/slides/slide10.xml".to_string(),
            "ppt/slides/slide2.xml".to_string(),
            "ppt/slides/slide20.xml".to_string(),
            "ppt/slides/slide3.xml".to_string(),
            "ppt/slides/slide11.xml".to_string(),
        ];

        paths.sort_by(|a, b| {
            fn slide_num(s: &str) -> u32 {
                s.rsplit('/')
                    .next()
                    .unwrap_or("")
                    .strip_prefix("slide")
                    .unwrap_or("")
                    .strip_suffix(".xml")
                    .unwrap_or("")
                    .parse()
                    .unwrap_or(u32::MAX)
            }
            slide_num(a).cmp(&slide_num(b))
        });

        assert_eq!(
            paths,
            vec![
                "ppt/slides/slide1.xml",
                "ppt/slides/slide2.xml",
                "ppt/slides/slide3.xml",
                "ppt/slides/slide10.xml",
                "ppt/slides/slide11.xml",
                "ppt/slides/slide20.xml",
            ]
        );
    }
}
