//! DOCX table property types and parsing for Word document extraction.
//!
//! This module provides comprehensive support for parsing table-level, row-level, and cell-level
//! properties from OOXML `<w:tblPr>`, `<w:trPr>`, and `<w:tcPr>` elements using streaming XML parsing.

use crate::extractors::security::{SecurityBudget, SecurityError};
use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};
use serde::{Deserialize, Serialize};

/// Table-level properties from `<w:tblPr>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct TableProperties {
    /// Style ID of the table style applied to this table.
    pub style_id: Option<String>,
    /// Total table width specification.
    pub width: Option<TableWidth>,
    /// Horizontal alignment: `"left"`, `"center"`, or `"right"`.
    pub alignment: Option<String>,
    /// Table layout algorithm: `"fixed"` or `"autofit"`.
    pub layout: Option<String>,
    /// Conditional formatting flags for header/banded rows/columns.
    pub look: Option<TableLook>,
    /// Table outer and inner border definitions.
    pub borders: Option<TableBorders>,
    /// Default cell margins applied to all cells in the table.
    pub cell_margins: Option<CellMargins>,
    /// Table indentation from the leading margin.
    pub indent: Option<TableWidth>,
    /// Table caption text (from `<w:tblCaption>`).
    pub caption: Option<String>,
}

/// Width specification used for tables and cells.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TableWidth {
    /// Numeric width value; interpretation depends on `width_type`.
    pub value: i32,
    /// Width unit: `"dxa"` (twips), `"pct"` (50ths of a percent), `"auto"`, or `"nil"`.
    pub width_type: String,
}

/// Table look bitmask/flags controlling conditional formatting bands.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct TableLook {
    /// Apply first-row conditional formatting.
    pub first_row: bool,
    /// Apply last-row conditional formatting.
    pub last_row: bool,
    /// Apply first-column conditional formatting.
    pub first_column: bool,
    /// Apply last-column conditional formatting.
    pub last_column: bool,
    /// Suppress horizontal banding.
    pub no_h_band: bool,
    /// Suppress vertical banding.
    pub no_v_band: bool,
}

/// Borders for a table (6 borders: top, bottom, left, right, insideH, insideV).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct TableBorders {
    /// Top outer border.
    pub top: Option<BorderStyle>,
    /// Bottom outer border.
    pub bottom: Option<BorderStyle>,
    /// Left outer border.
    pub left: Option<BorderStyle>,
    /// Right outer border.
    pub right: Option<BorderStyle>,
    /// Horizontal inner borders between rows.
    pub inside_h: Option<BorderStyle>,
    /// Vertical inner borders between columns.
    pub inside_v: Option<BorderStyle>,
}

/// A single border specification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct BorderStyle {
    /// Border line style: `"single"`, `"double"`, `"dashed"`, `"dotted"`, `"none"`, etc.
    pub style: String,
    /// Border thickness in eighths of a point.
    pub size: Option<i32>,
    /// Border color as a hex RGB string (e.g. `"2F5496"`) or `"auto"`.
    pub color: Option<String>,
    /// Spacing between the border and the cell contents in points.
    pub space: Option<i32>,
}

/// Cell margins (used for both table-level defaults and per-cell overrides).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct CellMargins {
    /// Top cell margin in twips.
    pub top: Option<i32>,
    /// Bottom cell margin in twips.
    pub bottom: Option<i32>,
    /// Left cell margin in twips.
    pub left: Option<i32>,
    /// Right cell margin in twips.
    pub right: Option<i32>,
}

/// Row-level properties from `<w:trPr>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct RowProperties {
    /// Row height in twips.
    pub height: Option<i32>,
    /// Height rule: `"auto"`, `"atLeast"`, or `"exact"`.
    pub height_rule: Option<String>,
    /// Whether this row acts as a repeating table header.
    pub is_header: bool,
    /// Whether this row may be split across a page break.
    pub cant_split: bool,
}

/// Cell-level properties from `<w:tcPr>`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct CellProperties {
    /// Cell width specification.
    pub width: Option<TableWidth>,
    /// Number of grid columns this cell spans (default 1).
    pub grid_span: Option<u32>,
    /// Vertical merge state for this cell.
    pub v_merge: Option<VerticalMerge>,
    /// Per-cell border overrides.
    pub borders: Option<CellBorders>,
    /// Cell background shading.
    pub shading: Option<CellShading>,
    /// Per-cell margin overrides.
    pub margins: Option<CellMargins>,
    /// Vertical text alignment: `"top"`, `"center"`, or `"bottom"`.
    pub vertical_align: Option<String>,
    /// Text direction: `"lrTb"`, `"tbRl"`, or `"btLr"`.
    pub text_direction: Option<String>,
    /// Whether cell content wraps across lines.
    pub no_wrap: bool,
}

/// Vertical merge state for a table cell.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum VerticalMerge {
    /// This cell starts a new vertically-merged group.
    Restart,
    /// This cell continues a vertically-merged group started above.
    Continue,
}

/// Per-cell borders (4 sides).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct CellBorders {
    /// Top cell border.
    pub top: Option<BorderStyle>,
    /// Bottom cell border.
    pub bottom: Option<BorderStyle>,
    /// Left (start in LTR) cell border.
    pub left: Option<BorderStyle>,
    /// Right (end in LTR) cell border.
    pub right: Option<BorderStyle>,
}

/// Cell shading/background fill.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[cfg_attr(alef, alef(skip))]
pub struct CellShading {
    /// Background fill color as hex RGB or `"auto"`.
    pub fill: Option<String>,
    /// Pattern foreground color.
    pub color: Option<String>,
    /// Pattern type: `"clear"`, `"solid"`, `"pct10"`, etc.
    pub val: Option<String>,
}

/// Column widths from `<w:tblGrid>`.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TableGrid {
    /// Ordered list of column widths in twips.
    pub columns: Vec<i32>,
}

/// Parse table-level properties from streaming XML reader.
///
/// Expects the reader to be positioned just after the `<w:tblPr>` start tag.
/// Reads all child elements until the matching `</w:tblPr>` end tag.
///
/// Threads `budget` through every event this function (and the delegating
/// helpers it calls) reads, so nesting inside `w:tblPr` is measured against
/// the caller's depth cap instead of passing through unaccounted (GH#384).
pub(crate) fn parse_table_properties(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<TableProperties, SecurityError> {
    let mut props = TableProperties::default();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"tblStyle" => {
                        props.style_id = get_attribute(&e, b"val");
                    }
                    b"tblW" => {
                        props.width = parse_width_element(&e);
                    }
                    b"jc" => {
                        props.alignment = get_attribute(&e, b"val");
                    }
                    b"tblLayout" => {
                        props.layout = get_attribute(&e, b"type");
                    }
                    b"tblLook" => {
                        props.look = Some(parse_table_look(&e));
                    }
                    b"tblBorders" => {
                        props.borders = Some(parse_table_borders(reader));
                        // `parse_table_borders` reads through its own `</w:tblBorders>`
                        // without touching `budget`; refund the enter above or every
                        // `tblBorders` child leaks one depth level.
                        budget.leave();
                    }
                    b"tblCellMar" => {
                        props.cell_margins = Some(parse_cell_margins_element(reader));
                        // Same as `tblBorders`: consumes its own end tag.
                        budget.leave();
                    }
                    b"tblInd" => {
                        props.indent = parse_width_element(&e);
                    }
                    b"tblCaption" => {
                        props.caption = get_attribute(&e, b"val");
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"tblStyle" => {
                        props.style_id = get_attribute(&e, b"val");
                    }
                    b"tblW" => {
                        props.width = parse_width_element(&e);
                    }
                    b"jc" => {
                        props.alignment = get_attribute(&e, b"val");
                    }
                    b"tblLayout" => {
                        props.layout = get_attribute(&e, b"type");
                    }
                    b"tblLook" => {
                        props.look = Some(parse_table_look(&e));
                    }
                    b"tblInd" => {
                        props.indent = parse_width_element(&e);
                    }
                    b"tblCaption" => {
                        props.caption = get_attribute(&e, b"val");
                    }
                    b"tblBorders" => {
                        props.borders = Some(TableBorders::default());
                    }
                    b"tblCellMar" => {
                        props.cell_margins = Some(CellMargins::default());
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::End(e)) => {
                budget.leave();
                if e.local_name().as_ref() as &[u8] == b"tblPr" {
                    break;
                }
                buf.clear();
            }
            Ok(Event::Eof) => break,
            _ => {
                buf.clear();
            }
        }
    }

    Ok(props)
}

/// Parse row-level properties from streaming XML reader.
///
/// Expects the reader to be positioned just after the `<w:trPr>` start tag.
///
/// Threads `budget` through every event so nesting inside `w:trPr` is
/// measured against the caller's depth cap (GH#384).
pub(crate) fn parse_row_properties(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<RowProperties, SecurityError> {
    let mut props = RowProperties::default();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"trHeight" => {
                        props.height = get_attribute_int(&e, b"val");
                        props.height_rule = get_attribute(&e, b"hRule");
                    }
                    b"tblHeader" => {
                        props.is_header = is_toggle_on(&e);
                    }
                    b"cantSplit" => {
                        props.cant_split = is_toggle_on(&e);
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"trHeight" => {
                        props.height = get_attribute_int(&e, b"val");
                        props.height_rule = get_attribute(&e, b"hRule");
                    }
                    b"tblHeader" => {
                        props.is_header = is_toggle_on(&e);
                    }
                    b"cantSplit" => {
                        props.cant_split = is_toggle_on(&e);
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::End(e)) => {
                budget.leave();
                if e.local_name().as_ref() as &[u8] == b"trPr" {
                    break;
                }
                buf.clear();
            }
            Ok(Event::Eof) => break,
            _ => {
                buf.clear();
            }
        }
    }

    Ok(props)
}

/// Parse cell-level properties from streaming XML reader.
///
/// Expects the reader to be positioned just after the `<w:tcPr>` start tag.
///
/// Threads `budget` through every event this function (and the delegating
/// helpers it calls) reads, so nesting inside `w:tcPr` is measured against
/// the caller's depth cap instead of passing through unaccounted (GH#384).
pub(crate) fn parse_cell_properties(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<CellProperties, SecurityError> {
    let mut props = CellProperties::default();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"tcW" => {
                        props.width = parse_width_element(&e);
                    }
                    b"gridSpan" => {
                        props.grid_span = get_attribute_u32(&e, b"val");
                    }
                    b"vMerge" => {
                        props.v_merge = Some(parse_vmerge(&e));
                    }
                    b"tcBorders" => {
                        props.borders = Some(parse_cell_borders(reader));
                        // `parse_cell_borders` reads through its own `</w:tcBorders>`
                        // without touching `budget`; refund the enter above.
                        budget.leave();
                    }
                    b"shd" => {
                        props.shading = Some(parse_cell_shading(&e));
                    }
                    b"tcMar" => {
                        props.margins = Some(parse_cell_margins_element(reader));
                        // `parse_cell_margins_element` reads through its own
                        // `</w:tcMar>` without touching `budget`; refund the enter above.
                        budget.leave();
                    }
                    b"vAlign" => {
                        props.vertical_align = get_attribute(&e, b"val");
                    }
                    b"textDirection" => {
                        props.text_direction = get_attribute(&e, b"val");
                    }
                    b"noWrap" => {
                        props.no_wrap = is_toggle_on(&e);
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"tcW" => {
                        props.width = parse_width_element(&e);
                    }
                    b"gridSpan" => {
                        props.grid_span = get_attribute_u32(&e, b"val");
                    }
                    b"vMerge" => {
                        props.v_merge = Some(parse_vmerge(&e));
                    }
                    b"shd" => {
                        props.shading = Some(parse_cell_shading(&e));
                    }
                    b"vAlign" => {
                        props.vertical_align = get_attribute(&e, b"val");
                    }
                    b"textDirection" => {
                        props.text_direction = get_attribute(&e, b"val");
                    }
                    b"noWrap" => {
                        props.no_wrap = is_toggle_on(&e);
                    }
                    b"tcBorders" => {
                        props.borders = Some(CellBorders::default());
                    }
                    b"tcMar" => {
                        props.margins = Some(CellMargins::default());
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::End(e)) => {
                budget.leave();
                if e.local_name().as_ref() as &[u8] == b"tcPr" {
                    break;
                }
                buf.clear();
            }
            Ok(Event::Eof) => break,
            _ => {
                buf.clear();
            }
        }
    }

    Ok(props)
}

/// Parse table grid (column widths) from streaming XML reader.
///
/// Expects the reader to be positioned just after the `<w:tblGrid>` start tag.
///
/// Threads `budget` through every event so nesting inside `w:tblGrid` is
/// measured against the caller's depth cap (GH#384).
pub(crate) fn parse_table_grid(
    reader: &mut Reader<&[u8]>,
    budget: &mut SecurityBudget,
) -> Result<TableGrid, SecurityError> {
    let mut grid = TableGrid::default();
    let mut buf = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                if e.local_name().as_ref() as &[u8] == b"gridCol"
                    && let Some(width) = get_attribute_int(&e, b"w")
                {
                    grid.columns.push(width);
                }
                buf.clear();
            }
            Ok(Event::Empty(e)) => {
                if e.local_name().as_ref() as &[u8] == b"gridCol"
                    && let Some(width) = get_attribute_int(&e, b"w")
                {
                    grid.columns.push(width);
                }
                buf.clear();
            }
            Ok(Event::End(e)) => {
                budget.leave();
                if e.local_name().as_ref() as &[u8] == b"tblGrid" {
                    break;
                }
                buf.clear();
            }
            Ok(Event::Eof) => break,
            _ => {
                buf.clear();
            }
        }
    }

    Ok(grid)
}

/// Helper: Check if an OOXML on/off toggle element is enabled.
/// Handles `<w:foo/>` (on), `<w:foo w:val="1"/>` (on), `<w:foo w:val="0"/>` (off),
/// `<w:foo w:val="true"/>` (on), `<w:foo w:val="false"/>` (off).
fn is_toggle_on(e: &BytesStart) -> bool {
    !matches!(
        get_attribute(e, b"val").as_deref(),
        Some("0") | Some("false") | Some("off")
    )
}

/// Helper: Parse border element from attributes.
fn parse_border_element(e: &BytesStart) -> BorderStyle {
    BorderStyle {
        style: get_attribute(e, b"val").unwrap_or_default(),
        size: get_attribute_int(e, b"sz"),
        color: get_attribute(e, b"color"),
        space: get_attribute_int(e, b"space"),
    }
}

/// Helper: Parse width element from attributes.
fn parse_width_element(e: &BytesStart) -> Option<TableWidth> {
    get_attribute_int(e, b"w").map(|value| TableWidth {
        value,
        width_type: get_attribute(e, b"type").unwrap_or_default(),
    })
}

/// Helper: Parse table look from attributes.
/// Handles both OOXML 2012+ individual boolean attributes and legacy hex bitmask.
fn parse_table_look(e: &BytesStart) -> TableLook {
    let mut look = TableLook::default();

    let has_individual = get_attribute(e, b"firstRow").is_some()
        || get_attribute(e, b"lastRow").is_some()
        || get_attribute(e, b"firstColumn").is_some()
        || get_attribute(e, b"lastColumn").is_some()
        || get_attribute(e, b"noHBand").is_some()
        || get_attribute(e, b"noVBand").is_some();

    if has_individual {
        look.first_row = get_attribute(e, b"firstRow").as_deref() == Some("1");
        look.last_row = get_attribute(e, b"lastRow").as_deref() == Some("1");
        look.first_column = get_attribute(e, b"firstColumn").as_deref() == Some("1");
        look.last_column = get_attribute(e, b"lastColumn").as_deref() == Some("1");
        look.no_h_band = get_attribute(e, b"noHBand").as_deref() == Some("1");
        look.no_v_band = get_attribute(e, b"noVBand").as_deref() == Some("1");
    } else if let Some(val_str) = get_attribute(e, b"val")
        && let Ok(mask) = i32::from_str_radix(&val_str, 16)
    {
        look.first_row = (mask & 0x0020) != 0;
        look.last_row = (mask & 0x0040) != 0;
        look.first_column = (mask & 0x0080) != 0;
        look.last_column = (mask & 0x0100) != 0;
        look.no_h_band = (mask & 0x0200) != 0;
        look.no_v_band = (mask & 0x0400) != 0;
    }

    look
}

/// Helper: Parse vertical merge state.
fn parse_vmerge(e: &BytesStart) -> VerticalMerge {
    match get_attribute(e, b"val") {
        Some(val) if val == "restart" => VerticalMerge::Restart,
        _ => VerticalMerge::Continue,
    }
}

/// Helper: Parse cell shading from attributes.
fn parse_cell_shading(e: &BytesStart) -> CellShading {
    CellShading {
        fill: get_attribute(e, b"fill"),
        color: get_attribute(e, b"color"),
        val: get_attribute(e, b"val"),
    }
}

/// Helper: Parse table borders container element.
fn parse_table_borders(reader: &mut Reader<&[u8]>) -> TableBorders {
    let mut borders = TableBorders::default();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"top" => {
                        borders.top = Some(parse_border_element(&e));
                    }
                    b"bottom" => {
                        borders.bottom = Some(parse_border_element(&e));
                    }
                    b"left" | b"start" => {
                        borders.left = Some(parse_border_element(&e));
                    }
                    b"right" | b"end" => {
                        borders.right = Some(parse_border_element(&e));
                    }
                    b"insideH" => {
                        borders.inside_h = Some(parse_border_element(&e));
                    }
                    b"insideV" => {
                        borders.inside_v = Some(parse_border_element(&e));
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"top" => {
                        borders.top = Some(parse_border_element(&e));
                    }
                    b"bottom" => {
                        borders.bottom = Some(parse_border_element(&e));
                    }
                    b"left" | b"start" => {
                        borders.left = Some(parse_border_element(&e));
                    }
                    b"right" | b"end" => {
                        borders.right = Some(parse_border_element(&e));
                    }
                    b"insideH" => {
                        borders.inside_h = Some(parse_border_element(&e));
                    }
                    b"insideV" => {
                        borders.inside_v = Some(parse_border_element(&e));
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() as &[u8] == b"tblBorders" {
                    break;
                }
                buf.clear();
            }
            Ok(Event::Eof) => break,
            _ => {
                buf.clear();
            }
        }
    }

    borders
}

/// Helper: Parse cell borders container element.
fn parse_cell_borders(reader: &mut Reader<&[u8]>) -> CellBorders {
    let mut borders = CellBorders::default();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"top" => {
                        borders.top = Some(parse_border_element(&e));
                    }
                    b"bottom" => {
                        borders.bottom = Some(parse_border_element(&e));
                    }
                    b"left" | b"start" => {
                        borders.left = Some(parse_border_element(&e));
                    }
                    b"right" | b"end" => {
                        borders.right = Some(parse_border_element(&e));
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"top" => {
                        borders.top = Some(parse_border_element(&e));
                    }
                    b"bottom" => {
                        borders.bottom = Some(parse_border_element(&e));
                    }
                    b"left" | b"start" => {
                        borders.left = Some(parse_border_element(&e));
                    }
                    b"right" | b"end" => {
                        borders.right = Some(parse_border_element(&e));
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() as &[u8] == b"tcBorders" {
                    break;
                }
                buf.clear();
            }
            Ok(Event::Eof) => break,
            _ => {
                buf.clear();
            }
        }
    }

    borders
}

/// Helper: Parse cell margins container element.
fn parse_cell_margins_element(reader: &mut Reader<&[u8]>) -> CellMargins {
    let mut margins = CellMargins::default();
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"top" => {
                        margins.top = get_attribute_int(&e, b"w");
                    }
                    b"bottom" => {
                        margins.bottom = get_attribute_int(&e, b"w");
                    }
                    b"left" | b"start" => {
                        margins.left = get_attribute_int(&e, b"w");
                    }
                    b"right" | b"end" => {
                        margins.right = get_attribute_int(&e, b"w");
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::Empty(e)) => {
                let local_name = e.local_name();
                match local_name.as_ref() {
                    b"top" => {
                        margins.top = get_attribute_int(&e, b"w");
                    }
                    b"bottom" => {
                        margins.bottom = get_attribute_int(&e, b"w");
                    }
                    b"left" | b"start" => {
                        margins.left = get_attribute_int(&e, b"w");
                    }
                    b"right" | b"end" => {
                        margins.right = get_attribute_int(&e, b"w");
                    }
                    _ => {}
                }
                buf.clear();
            }
            Ok(Event::End(e)) => {
                if e.local_name().as_ref() as &[u8] == b"tblCellMar" || e.local_name().as_ref() as &[u8] == b"tcMar" {
                    break;
                }
                buf.clear();
            }
            Ok(Event::Eof) => break,
            _ => {
                buf.clear();
            }
        }
    }

    margins
}

/// Helper: Extract string attribute value.
/// Uses local_name() to handle namespace-prefixed attributes (e.g., `w:val` matches `val`).
fn get_attribute(e: &BytesStart, key: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|attr| attr.key.local_name().as_ref() as &[u8] == key)
        .and_then(|attr| {
            let raw = std::str::from_utf8(&attr.value).ok()?;
            quick_xml::escape::unescape(raw).ok().map(|s| s.into_owned())
        })
}

/// Helper: Extract and parse integer attribute value.
fn get_attribute_int(e: &BytesStart, key: &[u8]) -> Option<i32> {
    get_attribute(e, key).and_then(|s| s.parse().ok())
}

/// Helper: Extract and parse unsigned integer attribute value.
fn get_attribute_u32(e: &BytesStart, key: &[u8]) -> Option<u32> {
    get_attribute(e, key).and_then(|s| s.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_table_properties_full() {
        let xml = r#"<w:tblPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:tblStyle w:val="TableGrid"/>
            <w:tblW w:w="5000" w:type="dxa"/>
            <w:jc w:val="center"/>
            <w:tblLayout w:type="fixed"/>
            <w:tblLook w:val="0460"/>
            <w:tblBorders>
                <w:top w:val="single" w:sz="12" w:color="000000" w:space="0"/>
                <w:bottom w:val="single" w:sz="12" w:color="000000" w:space="0"/>
            </w:tblBorders>
            <w:tblCellMar>
                <w:top w:w="0" w:type="dxa"/>
                <w:left w:w="108" w:type="dxa"/>
            </w:tblCellMar>
            <w:tblInd w:w="108" w:type="dxa"/>
        </w:tblPr>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let mut budget = SecurityBudget::with_defaults();
        let props = parse_table_properties(&mut reader, &mut budget).unwrap();

        assert_eq!(props.style_id, Some("TableGrid".to_string()));
        assert_eq!(
            props.width,
            Some(TableWidth {
                value: 5000,
                width_type: "dxa".to_string()
            })
        );
        assert_eq!(props.alignment, Some("center".to_string()));
        assert_eq!(props.layout, Some("fixed".to_string()));
        assert!(props.look.is_some());
        assert!(props.borders.is_some());
        assert!(props.cell_margins.is_some());
        assert_eq!(
            props.indent,
            Some(TableWidth {
                value: 108,
                width_type: "dxa".to_string()
            })
        );
    }

    #[test]
    fn test_parse_row_properties() {
        let xml = r#"<w:trPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:trHeight w:val="720" w:hRule="atLeast"/>
            <w:tblHeader/>
            <w:cantSplit/>
        </w:trPr>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let mut budget = SecurityBudget::with_defaults();
        let props = parse_row_properties(&mut reader, &mut budget).unwrap();

        assert_eq!(props.height, Some(720));
        assert_eq!(props.height_rule, Some("atLeast".to_string()));
        assert!(props.is_header);
        assert!(props.cant_split);
    }

    #[test]
    fn test_parse_cell_properties_merged() {
        let xml = r#"<w:tcPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:tcW w:w="2000" w:type="dxa"/>
            <w:gridSpan w:val="3"/>
            <w:vMerge w:val="restart"/>
        </w:tcPr>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let mut budget = SecurityBudget::with_defaults();
        let props = parse_cell_properties(&mut reader, &mut budget).unwrap();

        assert_eq!(
            props.width,
            Some(TableWidth {
                value: 2000,
                width_type: "dxa".to_string()
            })
        );
        assert_eq!(props.grid_span, Some(3));
        assert_eq!(props.v_merge, Some(VerticalMerge::Restart));
    }

    #[test]
    fn test_parse_cell_properties_shading() {
        let xml = r#"<w:tcPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/>
            <w:tcBorders>
                <w:top w:val="single" w:sz="8" w:color="000000"/>
                <w:left w:val="single" w:sz="8" w:color="000000"/>
            </w:tcBorders>
            <w:vAlign w:val="center"/>
        </w:tcPr>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let mut budget = SecurityBudget::with_defaults();
        let props = parse_cell_properties(&mut reader, &mut budget).unwrap();

        assert!(props.shading.is_some());
        let shading = props.shading.unwrap();
        assert_eq!(shading.fill, Some("D9E2F3".to_string()));
        assert_eq!(shading.color, Some("auto".to_string()));
        assert_eq!(shading.val, Some("clear".to_string()));

        assert!(props.borders.is_some());
        assert_eq!(props.vertical_align, Some("center".to_string()));
    }

    #[test]
    fn test_parse_table_grid() {
        let xml = r#"<w:tblGrid xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:gridCol w:w="2500"/>
            <w:gridCol w:w="2500"/>
            <w:gridCol w:w="2000"/>
            <w:gridCol w:w="2000"/>
        </w:tblGrid>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let mut budget = SecurityBudget::with_defaults();
        let grid = parse_table_grid(&mut reader, &mut budget).unwrap();

        assert_eq!(grid.columns, vec![2500, 2500, 2000, 2000]);
    }

    #[test]
    fn test_parse_table_look() {
        let xml = r#"<w:tblLook xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" w:val="0460"/>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        let event = reader.read_event_into(&mut buf).unwrap();

        if let Event::Empty(e) = event {
            let look = parse_table_look(&e);

            assert!(look.first_row);
            assert!(look.last_row);
            assert!(!look.first_column);
        } else {
            panic!("Expected Empty event");
        }
    }

    #[test]
    fn test_vmerge_continue() {
        let xml = r#"<w:tcPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:vMerge/>
        </w:tcPr>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let mut budget = SecurityBudget::with_defaults();
        let props = parse_cell_properties(&mut reader, &mut budget).unwrap();

        assert_eq!(props.v_merge, Some(VerticalMerge::Continue));
    }

    #[test]
    fn test_empty_table_properties() {
        let xml = r#"<w:tblPr xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"/>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let props = TableProperties::default();

        assert!(props.style_id.is_none());
        assert!(props.width.is_none());
        assert!(props.alignment.is_none());
    }

    #[test]
    fn test_cell_margins() {
        let xml = r#"<w:tblCellMar xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:top w:w="100" w:type="dxa"/>
            <w:bottom w:w="100" w:type="dxa"/>
            <w:left w:w="50" w:type="dxa"/>
            <w:right w:w="50" w:type="dxa"/>
        </w:tblCellMar>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let margins = parse_cell_margins_element(&mut reader);

        assert_eq!(margins.top, Some(100));
        assert_eq!(margins.bottom, Some(100));
        assert_eq!(margins.left, Some(50));
        assert_eq!(margins.right, Some(50));
    }

    #[test]
    fn test_border_styles() {
        let xml = r#"<w:tblBorders xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:top w:val="single" w:sz="12" w:color="FF0000" w:space="0"/>
            <w:bottom w:val="double" w:sz="24" w:color="0000FF" w:space="1"/>
            <w:left w:val="dashed" w:sz="8" w:color="auto"/>
            <w:right w:val="dotted" w:sz="4"/>
        </w:tblBorders>"#;

        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);

        let mut buf = Vec::new();
        reader.read_event_into(&mut buf).unwrap();
        buf.clear();

        let borders = parse_table_borders(&mut reader);

        assert!(borders.top.is_some());
        let top = borders.top.unwrap();
        assert_eq!(top.style, "single");
        assert_eq!(top.size, Some(12));
        assert_eq!(top.color, Some("FF0000".to_string()));
        assert_eq!(top.space, Some(0));

        assert!(borders.bottom.is_some());
        let bottom = borders.bottom.unwrap();
        assert_eq!(bottom.style, "double");
        assert_eq!(bottom.size, Some(24));

        assert!(borders.left.is_some());
        let left = borders.left.unwrap();
        assert_eq!(left.style, "dashed");
        assert_eq!(left.color, Some("auto".to_string()));

        assert!(borders.right.is_some());
        let right = borders.right.unwrap();
        assert_eq!(right.style, "dotted");
    }

    #[test]
    fn test_table_properties_round_trip_serialize() {
        let props = TableProperties {
            style_id: Some("TableGrid".to_string()),
            width: Some(TableWidth {
                value: 5000,
                width_type: "dxa".to_string(),
            }),
            alignment: Some("center".to_string()),
            layout: Some("fixed".to_string()),
            look: Some(TableLook {
                first_row: true,
                last_row: true,
                first_column: false,
                last_column: false,
                no_h_band: false,
                no_v_band: false,
            }),
            borders: None,
            cell_margins: None,
            indent: Some(TableWidth {
                value: 108,
                width_type: "dxa".to_string(),
            }),
            caption: None,
        };

        let json = serde_json::to_string(&props).unwrap();
        let deserialized: TableProperties = serde_json::from_str(&json).unwrap();

        assert_eq!(props, deserialized);
    }

    #[test]
    fn test_cell_properties_round_trip_serialize() {
        let props = CellProperties {
            width: Some(TableWidth {
                value: 2000,
                width_type: "dxa".to_string(),
            }),
            grid_span: Some(3),
            v_merge: Some(VerticalMerge::Restart),
            borders: None,
            shading: Some(CellShading {
                fill: Some("D9E2F3".to_string()),
                color: Some("auto".to_string()),
                val: Some("clear".to_string()),
            }),
            margins: None,
            vertical_align: Some("center".to_string()),
            text_direction: None,
            no_wrap: false,
        };

        let json = serde_json::to_string(&props).unwrap();
        let deserialized: CellProperties = serde_json::from_str(&json).unwrap();

        assert_eq!(props, deserialized);
    }
}
