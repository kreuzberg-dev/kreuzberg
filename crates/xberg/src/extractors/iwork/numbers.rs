//! Apple Numbers (.numbers) extractor.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::error::XbergError;
use crate::extractors::iwork::{
    IwaExpansionBudget, collect_iwa_paths, extract_metadata_from_zip, push_member_parse_warning, read_iwa_file,
    validate_iwork_zip,
};
use crate::extractors::security::{SecurityBudget, SecurityLimits};
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ProcessingWarning;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use async_trait::async_trait;
use std::collections::HashMap;

/// Apple Numbers spreadsheet extractor.
///
/// Supports `.numbers` files (modern iWork format, 2013+).
///
/// Extracts table names and cell string values from the IWA container:
/// ZIP → Snappy → typed protobuf table storage. Output preserves the source
/// row and column structure.
#[cfg_attr(alef, alef(skip))]
pub struct NumbersExtractor;

impl NumbersExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for NumbersExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for NumbersExtractor {
    fn name(&self) -> &str {
        "iwork-numbers-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Apple Numbers (.numbers) text extraction via IWA container parser"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

/// Parsed Numbers tables and optional metadata.
struct NumbersData {
    /// Tables from every document sheet, in display order. The sheet name is
    /// `None` when no sheet context is available (e.g. the legacy text
    /// fallback, which has no notion of sheets).
    tables: Vec<(Option<String>, String, Vec<Vec<String>>)>,
    /// Metadata extracted from the ZIP archive.
    metadata: crate::types::metadata::Metadata,
    /// Warnings for IWA members that failed to parse (#106).
    warnings: Vec<ProcessingWarning>,
}

/// Parse a Numbers ZIP and extract all text from IWA files.
///
/// Numbers stores its content across many IWA files:
/// - `Index/CalculationEngine.iwa` — formula cells and sheet data
/// - `Index/Document.iwa` — document structure and sheet names
/// - `tables/DataStore.iwa` — table cell string values
///
/// The IWA object type identifiers below are stable identifiers embedded in
/// Numbers' archive metadata, not protobuf field numbers. ~keep
const DOCUMENT_ARCHIVE_TYPE: u32 = 1;
const SHEET_ARCHIVE_TYPE: u32 = 2;
const TABLE_INFO_ARCHIVE_TYPE: u32 = 6000;
const TABLE_MODEL_ARCHIVE_TYPE: u32 = 6001;
const TILE_ARCHIVE_TYPE: u32 = 6002;
const TABLE_DATA_LIST_TYPES: &[u32] = &[6005, 6201];
const RICH_TEXT_PAYLOAD_ARCHIVE_TYPE: u32 = 6218;
const TEXT_STORAGE_ARCHIVE_TYPE: u32 = 2001;
const CELL_STORAGE_VERSION: u8 = 5;
const EMPTY_CELL_TYPE: u8 = 0;
const NUMBER_CELL_TYPE: u8 = 2;
const TEXT_CELL_TYPE: u8 = 3;
const DATE_CELL_TYPE: u8 = 5;
const BOOLEAN_CELL_TYPE: u8 = 6;
const DURATION_CELL_TYPE: u8 = 7;
const ERROR_CELL_TYPE: u8 = 8;
const RICH_TEXT_CELL_TYPE: u8 = 9;
const CURRENCY_CELL_TYPE: u8 = 10;
const CELL_HEADER_LENGTH: usize = 12;
const CELL_DECIMAL_FLAG: u32 = 0x1;
const CELL_DOUBLE_FLAG: u32 = 0x2;
const CELL_DATE_FLAG: u32 = 0x4;
const CELL_STRING_FLAG: u32 = 0x8;
const CELL_RICH_TEXT_FLAG: u32 = 0x10;
const DECIMAL_VALUE_LENGTH: usize = 16;
const SCALAR_VALUE_LENGTH: usize = 8;
const STRING_KEY_LENGTH: usize = 4;
const OFFSET_ENTRY_LENGTH: usize = 2;
const DEFAULT_TILE_SIZE: usize = 256;
const WIDE_OFFSET_SCALE: usize = 4;
const DECIMAL128_EXPONENT_BIAS: i32 = 0x1820;
const IWORK_EPOCH_TO_UNIX_SECONDS: i64 = 978_307_200;
const SECONDS_PER_DAY: i64 = 86_400;

const OLD_V1_MAX_VERSION: u8 = 1;
const OLD_V3_MAX_VERSION: u8 = 3;
const OLD_V1_HEADER_LENGTH: usize = 8;
const OLD_CELL_TYPE_V1_V3_OFFSET: usize = 2;
const OLD_CELL_TYPE_V4_OFFSET: usize = 1;
const OLD_FLAGS_OFFSET: usize = 4;
const OLD_STRING_FLAG: u32 = 0x10;
const OLD_DOUBLE_FLAG: u32 = 0x20;
const OLD_DATE_FLAG: u32 = 0x40;
const OLD_RICH_TEXT_FLAG: u32 = 0x200;
const OLD_CELL_STYLE_FLAG: u32 = 0x2;
const OLD_TEXT_STYLE_FLAG: u32 = 0x80;
const OLD_CONDITIONAL_STYLE_FLAG: u32 = 0x400;
const OLD_CONDITIONAL_RULE_FLAG: u32 = 0x800;
const OLD_CURRENT_FORMAT_FLAG: u32 = 0x4;
const OLD_FORMULA_FLAG: u32 = 0x8;
const OLD_FORMULA_ERROR_FLAG: u32 = 0x100;
const OLD_COMMENT_FLAG: u32 = 0x1000;
const OLD_IMPORT_WARNING_FLAG: u32 = 0x2000;
const OLD_NUMBER_FORMAT_FLAG: u32 = 0x10000;
const OLD_CURRENCY_FORMAT_FLAG: u32 = 0x80000;
const OLD_DATE_FORMAT_FLAG: u32 = 0x20000;
const OLD_DURATION_FORMAT_FLAG: u32 = 0x40000;
const OLD_CONTROL_FORMAT_FLAG: u32 = 0x100000;
const OLD_CUSTOM_FORMAT_FLAG: u32 = 0x200000;
const OLD_BASE_FORMAT_FLAG: u32 = 0x400000;
const OLD_CHOICE_FORMAT_FLAG: u32 = 0x800000;

mod field {
    pub(super) const ARCHIVE_IDENTIFIER: u32 = 1;
    pub(super) const ARCHIVE_MESSAGE_INFO: u32 = 2;
    pub(super) const ARCHIVE_SHOULD_MERGE: u32 = 3;
    pub(super) const MESSAGE_TYPE: u32 = 1;
    pub(super) const MESSAGE_LENGTH: u32 = 3;
    pub(super) const MESSAGE_BASE_INDEX: u32 = 7;
    pub(super) const DOCUMENT_SHEET: u32 = 1;
    pub(super) const SHEET_NAME: u32 = 1;
    pub(super) const SHEET_DRAWABLE: u32 = 2;
    pub(super) const TABLE_INFO_MODEL: u32 = 2;
    pub(super) const TABLE_DATA_STORE: u32 = 4;
    pub(super) const TABLE_ROWS: u32 = 6;
    pub(super) const TABLE_COLUMNS: u32 = 7;
    pub(super) const TABLE_NAME: u32 = 8;
    pub(super) const DATA_STORE_TILES: u32 = 3;
    pub(super) const DATA_STORE_STRINGS: u32 = 4;
    pub(super) const DATA_STORE_RICH_TEXT: u32 = 17;
    pub(super) const DATA_LIST_ENTRY: u32 = 3;
    pub(super) const DATA_LIST_ENTRY_KEY: u32 = 1;
    pub(super) const DATA_LIST_ENTRY_STRING: u32 = 3;
    pub(super) const DATA_LIST_ENTRY_RICH_TEXT: u32 = 9;
    pub(super) const RICH_TEXT_STORAGE: u32 = 1;
    pub(super) const TEXT_STORAGE_TEXT: u32 = 3;
    pub(super) const TILE_STORAGE_TILE: u32 = 1;
    pub(super) const TILE_STORAGE_SIZE: u32 = 2;
    pub(super) const TILE_INDEX: u32 = 1;
    pub(super) const TILE_REFERENCE: u32 = 2;
    pub(super) const TILE_ROW_INFO: u32 = 5;
    pub(super) const ROW_INDEX: u32 = 1;
    pub(super) const ROW_CELL_STORAGE: u32 = 6;
    pub(super) const ROW_CELL_OFFSETS: u32 = 7;
    pub(super) const ROW_CELL_STORAGE_PRE_BNC: u32 = 3;
    pub(super) const ROW_CELL_OFFSETS_PRE_BNC: u32 = 4;
    pub(super) const ROW_HAS_WIDE_OFFSETS: u32 = 8;
}

mod wire {
    pub(super) const VARINT: u64 = 0;
    pub(super) const FIXED64: u64 = 1;
    pub(super) const LENGTH_DELIMITED: u64 = 2;
    pub(super) const FIXED32: u64 = 5;
    pub(super) const FIELD_NUMBER_SHIFT: u32 = 3;
    pub(super) const TYPE_MASK: u64 = 0x7;
    pub(super) const REFERENCE_TAG: u64 = 8;
    pub(super) const FIXED64_LENGTH: u64 = 8;
    pub(super) const FIXED32_LENGTH: u64 = 4;
    pub(super) const VARINT_VALUE_MASK: u8 = 0x7f;
    pub(super) const VARINT_CONTINUATION_BIT: u8 = 0x80;
    pub(super) const VARINT_BITS_PER_BYTE: u32 = 7;
    pub(super) const VARINT_MAX_BITS: u32 = 64;
}

#[derive(Debug)]
struct IwaObject {
    object_type: u32,
    payload: Vec<u8>,
    is_merge_patch: bool,
}

type IwaObjects = HashMap<u64, Vec<IwaObject>>;

#[derive(Clone, Copy, Debug)]
enum ProtoValue<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

#[derive(Clone, Copy, Debug)]
struct ProtoField<'a> {
    number: u32,
    value: ProtoValue<'a>,
}

#[derive(Clone, Copy, Debug)]
struct IwaMessageInfo {
    object_type: u32,
    payload_length: u64,
    base_index: Option<usize>,
}

/// Parse a Numbers ZIP using the small subset of the reverse-engineered schema
/// needed for sheet/table names, string dictionaries, tiles, rows, and cells.
fn parse_numbers(content: &[u8], limits: &SecurityLimits) -> Result<NumbersData> {
    validate_iwork_zip(content, limits)?;
    let structured = {
        let mut budget = SecurityBudget::for_iwork(limits);
        let mut expansion = IwaExpansionBudget::from_limits(limits);
        parse_numbers_structured(content, &mut budget, &mut expansion)
    };
    match structured {
        Ok(data) if !data.tables.is_empty() => Ok(data),
        Ok(_) => parse_numbers_legacy_fresh(content, limits),
        Err(error) if matches!(&error, XbergError::Security { .. }) => Err(error),
        Err(error) => {
            tracing::debug!(%error, "structured Numbers table parsing failed; using legacy text fallback");
            parse_numbers_legacy_fresh(content, limits)
        }
    }
}

fn parse_numbers_legacy_fresh(content: &[u8], limits: &SecurityLimits) -> Result<NumbersData> {
    // Fallback is an independent parse attempt; fresh budgets prevent double-accounting the same member. ~keep
    let mut budget = SecurityBudget::for_iwork(limits);
    let mut expansion = IwaExpansionBudget::from_limits(limits);
    parse_numbers_legacy(content, &mut budget, &mut expansion)
}

fn parse_numbers_structured(
    content: &[u8],
    budget: &mut SecurityBudget,
    expansion: &mut IwaExpansionBudget,
) -> Result<NumbersData> {
    let mut warnings: Vec<ProcessingWarning> = Vec::new();
    let objects = read_iwa_objects(content, budget, expansion, &mut warnings)?;
    let metadata = extract_metadata_from_zip(content);

    let sheets = document_sheets(&objects, budget, &mut warnings)?;
    let mut tables = Vec::new();
    for sheet in sheets {
        // Numbers sheet names are otherwise dropped entirely (#111). Carry the sheet name
        // alongside each table so rendering can surface it as its own heading, matching how
        // the xlsx/ods path (`extraction/excel.rs`) headings a sheet separately from its
        // table content rather than folding it into the table's title text.
        let sheet_name = (!sheet.name.is_empty()).then_some(sheet.name);
        for table_id in sheet.table_ids {
            if let Some((table_name, cells)) = parse_table(table_id, &objects, budget, &mut warnings)? {
                tables.push((sheet_name.clone(), table_name, cells));
            }
        }
    }

    Ok(NumbersData {
        tables,
        metadata,
        warnings,
    })
}

fn parse_numbers_legacy(
    content: &[u8],
    budget: &mut SecurityBudget,
    expansion: &mut IwaExpansionBudget,
) -> Result<NumbersData> {
    let iwa_paths = collect_iwa_paths(content)?;
    let metadata = extract_metadata_from_zip(content);
    let mut table_cells = Vec::new();
    let mut other_cells = Vec::new();
    let mut table_seen = std::collections::HashSet::new();
    let mut other_seen = std::collections::HashSet::new();
    let mut warnings: Vec<ProcessingWarning> = Vec::new();

    for path in iwa_paths {
        budget.step()?;
        let decompressed = match read_iwa_file(content, &path, expansion) {
            Ok(decompressed) => decompressed,
            Err(error) if matches!(&error, XbergError::Security { .. }) => return Err(error),
            Err(error) => {
                tracing::debug!(member = %path, %error, "skipping unreadable legacy iWork member");
                push_member_parse_warning(&mut warnings, &path, &error);
                continue;
            }
        };
        let texts = crate::extractors::iwork::extract_text_from_proto(&decompressed, budget)?;
        if path.contains("Table") || path.contains("DataStore") {
            append_legacy_cells(texts, &mut table_seen, &mut table_cells, budget)?;
        } else {
            append_legacy_cells(texts, &mut other_seen, &mut other_cells, budget)?;
        }
    }

    let mut tables = Vec::new();
    if !table_cells.is_empty() {
        tables.push((None, "Sheet Data".to_string(), table_cells));
    }
    if !other_cells.is_empty() {
        tables.push((None, "Document Info".to_string(), other_cells));
    }
    Ok(NumbersData {
        tables,
        metadata,
        warnings,
    })
}

fn append_legacy_cells(
    texts: Vec<String>,
    seen: &mut std::collections::HashSet<String>,
    cells: &mut Vec<Vec<String>>,
    budget: &mut SecurityBudget,
) -> Result<()> {
    for text in texts {
        // No byte-length floor: a single alphanumeric character can be real content
        // (a numeric answer, a unit label) — dropping it here was #107.
        if !text.chars().any(char::is_alphanumeric) || seen.contains(&text) {
            continue;
        }
        // Charge the cell before HashSet/row allocations so the limit prevents growth. ~keep
        budget.add_cells(1)?;
        seen.insert(text.clone());
        cells.push(vec![text]);
    }
    Ok(())
}

fn read_iwa_objects(
    content: &[u8],
    budget: &mut SecurityBudget,
    expansion: &mut IwaExpansionBudget,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<IwaObjects> {
    let mut objects: IwaObjects = HashMap::new();
    for path in collect_iwa_paths(content)? {
        budget.step()?;
        let decompressed = match read_iwa_file(content, &path, expansion) {
            Ok(data) => data,
            Err(error) if matches!(&error, XbergError::Security { .. }) => return Err(error),
            Err(error) => {
                tracing::debug!(member = %path, %error, "skipping unreadable iWork archive member");
                push_member_parse_warning(warnings, &path, &error);
                continue;
            }
        };
        let mut member_objects = HashMap::new();
        if let Err(error) = parse_iwa_segments(&decompressed, budget, &mut member_objects) {
            if matches!(&error, XbergError::Security { .. }) {
                return Err(error);
            }
            tracing::debug!(member = %path, %error, "skipping malformed iWork archive member");
            push_member_parse_warning(warnings, &path, &error);
            continue;
        }
        for (identifier, mut messages) in member_objects {
            objects.entry(identifier).or_default().append(&mut messages);
        }
    }
    reject_required_merge_patches(&objects)?;
    Ok(objects)
}

fn reject_required_merge_patches(objects: &IwaObjects) -> Result<()> {
    if objects.values().flatten().any(|object| object.is_merge_patch) {
        // Applying field-path protobuf diffs without the full schema could return stale tables. ~keep
        return Err(numbers_parse_error(
            "Numbers archive uses a merge patch that requires schema-aware reconstruction",
        ));
    }
    Ok(())
}

fn parse_iwa_segments(data: &[u8], budget: &mut SecurityBudget, objects: &mut IwaObjects) -> Result<()> {
    let mut position = 0usize;
    while position < data.len() {
        budget.step()?;
        let (identifier, should_merge, message_infos) = parse_iwa_segment_header(data, &mut position, budget)?;
        store_iwa_messages(data, &mut position, identifier, should_merge, &message_infos, objects)?;
    }
    Ok(())
}

fn parse_iwa_segment_header(
    data: &[u8],
    position: &mut usize,
    budget: &mut SecurityBudget,
) -> Result<(u64, bool, Vec<IwaMessageInfo>)> {
    let (header_length, prefix_length) = read_varint_at(data, *position)?;
    *position = position
        .checked_add(prefix_length)
        .ok_or_else(|| numbers_parse_error("IWA header offset overflow"))?;
    let header_end = checked_end(*position, header_length, data.len(), "IWA archive header")?;
    let header_fields = parse_proto_fields(&data[*position..header_end], budget)?;
    *position = header_end;
    let identifier = field_varint(&header_fields, field::ARCHIVE_IDENTIFIER)
        .ok_or_else(|| numbers_parse_error("IWA ArchiveInfo has no object identifier"))?;
    let should_merge = field_varint(&header_fields, field::ARCHIVE_SHOULD_MERGE).is_some_and(|value| value != 0);
    let message_infos = parse_iwa_message_infos(&header_fields, budget)?;
    Ok((identifier, should_merge, message_infos))
}

fn parse_iwa_message_infos(fields: &[ProtoField<'_>], budget: &mut SecurityBudget) -> Result<Vec<IwaMessageInfo>> {
    field_bytes(fields, field::ARCHIVE_MESSAGE_INFO)
        .map(|message| {
            let message_fields = parse_proto_fields(message, budget)?;
            let object_type = field_varint(&message_fields, field::MESSAGE_TYPE)
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| numbers_parse_error("IWA MessageInfo has no valid object type"))?;
            let payload_length = field_varint(&message_fields, field::MESSAGE_LENGTH)
                .ok_or_else(|| numbers_parse_error("IWA MessageInfo has no payload length"))?;
            let base_index = field_usize(&message_fields, field::MESSAGE_BASE_INDEX);
            Ok(IwaMessageInfo {
                object_type,
                payload_length,
                base_index,
            })
        })
        .collect()
}

fn store_iwa_messages(
    data: &[u8],
    position: &mut usize,
    identifier: u64,
    should_merge: bool,
    message_infos: &[IwaMessageInfo],
    objects: &mut IwaObjects,
) -> Result<()> {
    for message in message_infos {
        let payload_end = checked_end(*position, message.payload_length, data.len(), "IWA object payload")?;
        let is_merge_patch = should_merge && message.object_type == 0;
        let effective_type = effective_iwa_object_type(message, message_infos, is_merge_patch);
        if is_required_object_type(effective_type) {
            let payload = data[*position..payload_end].to_vec();
            // numbers-parser defines ordered bodies and base_message_index; retain all for fallback. ~keep
            objects.entry(identifier).or_default().push(IwaObject {
                object_type: effective_type,
                payload,
                is_merge_patch,
            });
        }
        *position = payload_end;
    }
    Ok(())
}

fn effective_iwa_object_type(message: &IwaMessageInfo, message_infos: &[IwaMessageInfo], is_merge_patch: bool) -> u32 {
    if !is_merge_patch {
        return message.object_type;
    }
    message
        .base_index
        .and_then(|index| message_infos.get(index))
        .map_or(0, |base| base.object_type)
}

fn is_required_object_type(object_type: u32) -> bool {
    matches!(
        object_type,
        DOCUMENT_ARCHIVE_TYPE
            | SHEET_ARCHIVE_TYPE
            | TABLE_INFO_ARCHIVE_TYPE
            | TABLE_MODEL_ARCHIVE_TYPE
            | TILE_ARCHIVE_TYPE
            | RICH_TEXT_PAYLOAD_ARCHIVE_TYPE
            | TEXT_STORAGE_ARCHIVE_TYPE
    ) || TABLE_DATA_LIST_TYPES.contains(&object_type)
}

fn object_for_type(objects: &IwaObjects, identifier: u64, object_type: u32) -> Option<&IwaObject> {
    objects
        .get(&identifier)?
        .iter()
        .rev()
        .find(|object| object.object_type == object_type && !object.is_merge_patch)
}

/// A document sheet's display name and the table archive ids drawn on it.
struct SheetTableRefs {
    name: String,
    table_ids: Vec<u64>,
}

/// Walk every sheet referenced from the document archive, resolving each
/// sheet's display name (#111) and the tables drawn on it.
///
/// A sheet's `SHEET_DRAWABLE` field lists every drawable placed on it — tables
/// *and* non-table shapes (text boxes, images, charts). Only tables are
/// resolved into structured data here; a drawable that resolves to some other
/// archive type is named in a `ProcessingWarning` (#111) instead of vanishing
/// silently, since xberg has no schema for reconstructing arbitrary iWork
/// drawables.
fn document_sheets(
    objects: &IwaObjects,
    budget: &mut SecurityBudget,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<Vec<SheetTableRefs>> {
    let document = objects
        .values()
        .find_map(|messages| {
            messages
                .iter()
                .rev()
                .find(|object| object.object_type == DOCUMENT_ARCHIVE_TYPE && !object.is_merge_patch)
        })
        .ok_or_else(|| numbers_parse_error("Numbers document archive is missing"))?;
    let document_fields = parse_proto_fields(&document.payload, budget)?;
    let mut sheets = Vec::new();
    let mut has_table = false;
    for sheet_reference in field_bytes(&document_fields, field::DOCUMENT_SHEET) {
        let Some(sheet_id) = reference_identifier(sheet_reference) else {
            continue;
        };
        let Some(sheet) = object_for_type(objects, sheet_id, SHEET_ARCHIVE_TYPE) else {
            continue;
        };
        let sheet_fields = parse_proto_fields(&sheet.payload, budget)?;
        let sheet_name = parse_sheet_name(&sheet_fields, budget)?;
        let (table_ids, skipped_types) = resolve_sheet_drawables(&sheet_fields, objects, budget)?;
        has_table |= !table_ids.is_empty();
        push_non_table_drawable_warning(warnings, &sheet_name, &skipped_types);
        sheets.push(SheetTableRefs {
            name: sheet_name,
            table_ids,
        });
    }
    if !has_table {
        return Err(numbers_parse_error("Numbers document has no readable table references"));
    }
    Ok(sheets)
}

/// Resolve a sheet's `SHEET_DRAWABLE` references into table archive ids,
/// collecting the archive type of every drawable that is not a table.
fn resolve_sheet_drawables(
    sheet_fields: &[ProtoField<'_>],
    objects: &IwaObjects,
    budget: &mut SecurityBudget,
) -> Result<(Vec<u64>, Vec<u32>)> {
    let mut table_ids = Vec::new();
    let mut skipped_types = Vec::new();
    for drawable in field_bytes(sheet_fields, field::SHEET_DRAWABLE) {
        let Some(drawable_id) = reference_identifier(drawable) else {
            continue;
        };
        if let Some(table_info) = object_for_type(objects, drawable_id, TABLE_INFO_ARCHIVE_TYPE) {
            let fields = parse_proto_fields(&table_info.payload, budget)?;
            if let Some(table_id) = field_bytes(&fields, field::TABLE_INFO_MODEL)
                .next()
                .and_then(reference_identifier)
            {
                table_ids.push(table_id);
            }
            continue;
        }
        if let Some(candidates) = objects.get(&drawable_id) {
            skipped_types.extend(
                candidates
                    .iter()
                    .filter(|object| !object.is_merge_patch)
                    .map(|object| object.object_type),
            );
        }
    }
    Ok((table_ids, skipped_types))
}

/// Record that a sheet placed one or more non-table drawables that xberg
/// cannot reconstruct (#111). Naming the archive type keeps the warning
/// actionable without inventing shape/image/text-box content xberg never
/// parsed.
fn push_non_table_drawable_warning(warnings: &mut Vec<ProcessingWarning>, sheet_name: &str, archive_types: &[u32]) {
    if archive_types.is_empty() {
        return;
    }
    let types = archive_types.iter().map(u32::to_string).collect::<Vec<_>>().join(", ");
    crate::core::diagnostics::push_warning(
        warnings,
        crate::extractors::iwork::IWORK_WARNING_SOURCE,
        format!(
            "Sheet '{sheet_name}' contains {} non-table drawable object(s) (archive type(s): {types}) that xberg \
             does not extract; only tables are supported",
            archive_types.len()
        ),
    );
}

fn parse_sheet_name(fields: &[ProtoField<'_>], budget: &mut SecurityBudget) -> Result<String> {
    let name = field_bytes(fields, field::SHEET_NAME)
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .unwrap_or("Sheet")
        .to_string();
    budget.check_entity(&name)?;
    budget.account_text(name.len())?;
    Ok(name)
}

fn parse_table(
    table_id: u64,
    objects: &IwaObjects,
    budget: &mut SecurityBudget,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<Option<(String, Vec<Vec<String>>)>> {
    let Some(model) = object_for_type(objects, table_id, TABLE_MODEL_ARCHIVE_TYPE) else {
        return Ok(None);
    };
    let fields = parse_proto_fields(&model.payload, budget)?;
    let name = parse_table_name(&fields, budget)?;
    let (rows, columns) = parse_table_dimensions(&fields, budget)?;
    if rows == 0 || columns == 0 {
        return Ok(None);
    }

    let Some(data_store) = field_bytes(&fields, field::TABLE_DATA_STORE).next() else {
        return Ok(None);
    };
    let data_store_fields = parse_proto_fields(data_store, budget)?;
    let (strings, rich_strings) = parse_table_dictionaries(&data_store_fields, objects, budget)?;

    let mut cells = vec![vec![String::new(); columns]; rows];
    if let Some(tile_storage) = field_bytes(&data_store_fields, field::DATA_STORE_TILES).next() {
        let mut context = TableFillContext {
            strings: &strings,
            rich_strings: &rich_strings,
            budget,
            table_name: &name,
            warnings,
        };
        fill_table_tiles(tile_storage, objects, &mut cells, &mut context)?;
    }
    Ok(Some((name, cells)))
}

fn parse_table_name(fields: &[ProtoField<'_>], budget: &mut SecurityBudget) -> Result<String> {
    let name = field_bytes(fields, field::TABLE_NAME)
        .next()
        .and_then(|value| std::str::from_utf8(value).ok())
        .unwrap_or("Table")
        .to_string();
    budget.check_entity(&name)?;
    budget.account_text(name.len())?;
    Ok(name)
}

fn parse_table_dimensions(fields: &[ProtoField<'_>], budget: &mut SecurityBudget) -> Result<(usize, usize)> {
    let rows = field_usize(fields, field::TABLE_ROWS).unwrap_or(0);
    let columns = field_usize(fields, field::TABLE_COLUMNS).unwrap_or(0);
    let cell_count = rows
        .checked_mul(columns)
        .ok_or_else(|| numbers_parse_error("Numbers table dimensions overflow"))?;
    budget.add_cells(cell_count)?;
    Ok((rows, columns))
}

fn parse_table_dictionaries(
    fields: &[ProtoField<'_>],
    objects: &IwaObjects,
    budget: &mut SecurityBudget,
) -> Result<(HashMap<i32, String>, HashMap<i32, String>)> {
    let strings = field_bytes(fields, field::DATA_STORE_STRINGS)
        .next()
        .and_then(reference_identifier)
        .and_then(|identifier| object_for_table_data_list(objects, identifier))
        .map(|object| parse_string_table(&object.payload, budget))
        .transpose()?
        .unwrap_or_default();
    let rich_strings = field_bytes(fields, field::DATA_STORE_RICH_TEXT)
        .next()
        .and_then(reference_identifier)
        .and_then(|identifier| object_for_table_data_list(objects, identifier))
        .map(|object| parse_rich_text_table(&object.payload, objects, budget))
        .transpose()?
        .unwrap_or_default();
    Ok((strings, rich_strings))
}

fn object_for_table_data_list(objects: &IwaObjects, identifier: u64) -> Option<&IwaObject> {
    TABLE_DATA_LIST_TYPES
        .iter()
        .find_map(|object_type| object_for_type(objects, identifier, *object_type))
}

fn parse_string_table(payload: &[u8], budget: &mut SecurityBudget) -> Result<HashMap<i32, String>> {
    let fields = parse_proto_fields(payload, budget)?;
    let mut strings = HashMap::new();
    for entry in field_bytes(&fields, field::DATA_LIST_ENTRY) {
        let entry_fields = parse_proto_fields(entry, budget)?;
        let Some(key) =
            field_varint(&entry_fields, field::DATA_LIST_ENTRY_KEY).and_then(|value| i32::try_from(value).ok())
        else {
            continue;
        };
        let Some(value) = field_bytes(&entry_fields, field::DATA_LIST_ENTRY_STRING)
            .next()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
        else {
            continue;
        };
        budget.check_entity(value)?;
        budget.account_text(value.len())?;
        strings.insert(key, value.to_string());
    }
    Ok(strings)
}

fn parse_rich_text_table(
    payload: &[u8],
    objects: &IwaObjects,
    budget: &mut SecurityBudget,
) -> Result<HashMap<i32, String>> {
    let fields = parse_proto_fields(payload, budget)?;
    let mut strings = HashMap::new();
    for entry in field_bytes(&fields, field::DATA_LIST_ENTRY) {
        let entry_fields = parse_proto_fields(entry, budget)?;
        let Some(key) =
            field_varint(&entry_fields, field::DATA_LIST_ENTRY_KEY).and_then(|value| i32::try_from(value).ok())
        else {
            continue;
        };
        let Some(payload_id) = field_bytes(&entry_fields, field::DATA_LIST_ENTRY_RICH_TEXT)
            .next()
            .and_then(reference_identifier)
        else {
            continue;
        };
        let Some(rich_payload) = object_for_type(objects, payload_id, RICH_TEXT_PAYLOAD_ARCHIVE_TYPE) else {
            continue;
        };
        let rich_fields = parse_proto_fields(&rich_payload.payload, budget)?;
        let Some(storage_id) = field_bytes(&rich_fields, field::RICH_TEXT_STORAGE)
            .next()
            .and_then(reference_identifier)
        else {
            continue;
        };
        let Some(storage) = object_for_type(objects, storage_id, TEXT_STORAGE_ARCHIVE_TYPE) else {
            continue;
        };
        let storage_fields = parse_proto_fields(&storage.payload, budget)?;
        let value = field_bytes(&storage_fields, field::TEXT_STORAGE_TEXT)
            .filter_map(|bytes| std::str::from_utf8(bytes).ok())
            .collect::<String>();
        if value.is_empty() {
            continue;
        }
        budget.check_entity(&value)?;
        budget.account_text(value.len())?;
        strings.insert(key, value);
    }
    Ok(strings)
}

/// Shared string dictionaries plus the mutable resource/diagnostic state threaded through
/// the table → tile → row → cell filling chain (`fill_table_tiles` → `fill_tile` → `fill_row`
/// → `parse_cell_value`).
///
/// Every function in that chain needs the same pieces of context alongside its own
/// tile/row/cell-specific arguments; grouping them here keeps each function's own argument
/// count small instead of repeating the same parameters at every level.
struct TableFillContext<'a> {
    /// Plain-text cell values from the table's string dictionary, keyed by `DATA_LIST_ENTRY_KEY`.
    strings: &'a HashMap<i32, String>,
    /// Rich-text cell values from the table's rich-text dictionary, keyed by `DATA_LIST_ENTRY_KEY`.
    rich_strings: &'a HashMap<i32, String>,
    /// Extraction resource budget, shared across the whole table parse.
    budget: &'a mut SecurityBudget,
    /// Display name of the table being parsed, used only in warning messages.
    table_name: &'a str,
    /// Non-fatal parse warnings collected across the whole table parse.
    warnings: &'a mut Vec<ProcessingWarning>,
}

fn fill_table_tiles(
    tile_storage: &[u8],
    objects: &IwaObjects,
    cells: &mut [Vec<String>],
    context: &mut TableFillContext<'_>,
) -> Result<()> {
    let storage_fields = parse_proto_fields(tile_storage, context.budget)?;
    let tile_size = field_usize(&storage_fields, field::TILE_STORAGE_SIZE).unwrap_or(DEFAULT_TILE_SIZE);
    for tile_entry in field_bytes(&storage_fields, field::TILE_STORAGE_TILE) {
        let tile_fields = parse_proto_fields(tile_entry, context.budget)?;
        let tile_index = field_usize(&tile_fields, field::TILE_INDEX).unwrap_or(0);
        let Some(tile_id) = field_bytes(&tile_fields, field::TILE_REFERENCE)
            .next()
            .and_then(reference_identifier)
        else {
            continue;
        };
        let Some(tile) = object_for_type(objects, tile_id, TILE_ARCHIVE_TYPE) else {
            continue;
        };
        let row_offset = tile_index
            .checked_mul(tile_size)
            .ok_or_else(|| numbers_parse_error("Numbers tile row offset overflow"))?;
        fill_tile(&tile.payload, row_offset, cells, context)?;
    }
    Ok(())
}

fn fill_tile(
    payload: &[u8],
    row_offset: usize,
    cells: &mut [Vec<String>],
    context: &mut TableFillContext<'_>,
) -> Result<()> {
    let fields = parse_proto_fields(payload, context.budget)?;
    for row_info in field_bytes(&fields, field::TILE_ROW_INFO) {
        let row_fields = parse_proto_fields(row_info, context.budget)?;
        let Some(row_index) =
            field_usize(&row_fields, field::ROW_INDEX).and_then(|index| row_offset.checked_add(index))
        else {
            continue;
        };
        let Some(row) = cells.get_mut(row_index) else {
            continue;
        };
        let modern_storage = field_bytes(&row_fields, field::ROW_CELL_STORAGE).next();
        let modern_offsets = field_bytes(&row_fields, field::ROW_CELL_OFFSETS).next();
        let (storage, offsets, wide_offsets) = match (modern_storage, modern_offsets) {
            (Some(storage), Some(offsets)) if !storage.is_empty() || !offsets.is_empty() => (
                storage,
                offsets,
                field_varint(&row_fields, field::ROW_HAS_WIDE_OFFSETS).is_some_and(|value| value != 0),
            ),
            _ => (
                field_bytes(&row_fields, field::ROW_CELL_STORAGE_PRE_BNC)
                    .next()
                    .unwrap_or_default(),
                field_bytes(&row_fields, field::ROW_CELL_OFFSETS_PRE_BNC)
                    .next()
                    .unwrap_or_default(),
                false,
            ),
        };
        fill_row(row, storage, offsets, wide_offsets, context)?;
    }
    Ok(())
}

fn fill_row(
    row: &mut [String],
    storage: &[u8],
    offsets: &[u8],
    wide_offsets: bool,
    context: &mut TableFillContext<'_>,
) -> Result<()> {
    let parsed_offsets = parse_cell_offsets(offsets, row.len(), wide_offsets)?;

    for (column, start) in parsed_offsets.iter().copied().enumerate() {
        let Some(start) = start else {
            continue;
        };
        let end = parsed_offsets[column + 1..]
            .iter()
            .flatten()
            .copied()
            .next()
            .unwrap_or(storage.len());
        if start > end || end > storage.len() {
            return Err(numbers_parse_error("Numbers cell storage offset is out of bounds"));
        }
        if let Some(value) = parse_cell_value(&storage[start..end], context)? {
            context.budget.account_text(value.len())?;
            row[column] = value;
        }
    }
    Ok(())
}

fn parse_cell_offsets(offsets: &[u8], column_count: usize, wide_offsets: bool) -> Result<Vec<Option<usize>>> {
    offsets
        .chunks_exact(OFFSET_ENTRY_LENGTH)
        .take(column_count)
        .map(|bytes| {
            let offset = u16::from_le_bytes([bytes[0], bytes[1]]);
            if offset == u16::MAX {
                Ok(None)
            } else {
                let scale = if wide_offsets { WIDE_OFFSET_SCALE } else { 1 };
                let expanded = usize::from(offset)
                    .checked_mul(scale)
                    .ok_or_else(|| numbers_parse_error("Numbers cell offset overflow"))?;
                Ok(Some(expanded))
            }
        })
        .collect()
}

fn parse_cell_value(storage: &[u8], context: &mut TableFillContext<'_>) -> Result<Option<String>> {
    let Some(version) = storage.first().copied() else {
        return Err(numbers_parse_error("Numbers cell storage is truncated"));
    };
    if version == CELL_STORAGE_VERSION {
        parse_v5_cell(storage, context.strings, context.rich_strings)
    } else if version <= 4 {
        parse_old_cell(
            storage,
            context.strings,
            context.rich_strings,
            context.table_name,
            context.warnings,
        )
    } else {
        tracing::debug!(version, "skipping unsupported Numbers cell storage version");
        Ok(None)
    }
}

fn parse_v5_cell(
    storage: &[u8],
    strings: &HashMap<i32, String>,
    rich_strings: &HashMap<i32, String>,
) -> Result<Option<String>> {
    if storage.len() < CELL_HEADER_LENGTH {
        return Err(numbers_parse_error("Numbers v5 cell storage is truncated"));
    }
    let cell_type = storage[1];
    let flags = u32::from_le_bytes([storage[8], storage[9], storage[10], storage[11]]);
    let mut cursor = CELL_HEADER_LENGTH;
    let decimal =
        take_flagged(storage, &mut cursor, flags, CELL_DECIMAL_FLAG, DECIMAL_VALUE_LENGTH)?.map(decode_decimal128);
    let double = take_flagged(storage, &mut cursor, flags, CELL_DOUBLE_FLAG, SCALAR_VALUE_LENGTH)?.map(decode_f64);
    let seconds = take_flagged(storage, &mut cursor, flags, CELL_DATE_FLAG, SCALAR_VALUE_LENGTH)?.map(decode_f64);
    let string_key = take_flagged(storage, &mut cursor, flags, CELL_STRING_FLAG, STRING_KEY_LENGTH)?.map(decode_i32);
    let rich_key = take_flagged(storage, &mut cursor, flags, CELL_RICH_TEXT_FLAG, STRING_KEY_LENGTH)?.map(decode_i32);

    // Source: numbers-parser cell.py::_from_storage defines the v5 layout and type IDs. ~keep
    Ok(match cell_type {
        EMPTY_CELL_TYPE | ERROR_CELL_TYPE => None,
        NUMBER_CELL_TYPE | CURRENCY_CELL_TYPE => decimal.or(double).map(format_scalar),
        TEXT_CELL_TYPE => string_key.and_then(|key| strings.get(&key).cloned()),
        DATE_CELL_TYPE => seconds.map(format_iwork_date),
        BOOLEAN_CELL_TYPE => double.map(|value| (value > 0.0).to_string()),
        DURATION_CELL_TYPE => double.map(|value| format!("{}s", format_scalar(value))),
        RICH_TEXT_CELL_TYPE => rich_key.and_then(|key| rich_strings.get(&key).cloned()),
        _ => {
            tracing::debug!(cell_type, "skipping unsupported Numbers v5 cell type");
            None
        }
    })
}

fn parse_old_cell(
    storage: &[u8],
    strings: &HashMap<i32, String>,
    rich_strings: &HashMap<i32, String>,
    table_name: &str,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<Option<String>> {
    let version = storage[0];
    let header_length = if version <= OLD_V1_MAX_VERSION {
        OLD_V1_HEADER_LENGTH
    } else {
        CELL_HEADER_LENGTH
    };
    if storage.len() < header_length {
        return Err(numbers_parse_error("Numbers legacy cell storage is truncated"));
    }
    let cell_type_offset = if version <= OLD_V3_MAX_VERSION {
        OLD_CELL_TYPE_V1_V3_OFFSET
    } else {
        OLD_CELL_TYPE_V4_OFFSET
    };
    let cell_type = storage[cell_type_offset];
    let flags = if version <= OLD_V1_MAX_VERSION {
        u32::from(u16::from_le_bytes([
            storage[OLD_FLAGS_OFFSET],
            storage[OLD_FLAGS_OFFSET + 1],
        ]))
    } else {
        let mut bytes = [0_u8; 4];
        bytes.copy_from_slice(&storage[OLD_FLAGS_OFFSET..OLD_FLAGS_OFFSET + 4]);
        u32::from_le_bytes(bytes)
    };
    let fields = parse_old_cell_fields(storage, header_length, flags)?;
    // A legacy formula/comment key is a flag bit only (#110): xberg has no schema
    // for the pre-BNC formula token stream or comment thread payload behind that
    // key, so the presence is surfaced via warning instead of guessing at text
    // the parser cannot actually decode.
    push_legacy_formula_comment_warning(warnings, table_name, &fields);

    // Source: https://oss.sheetjs.com/notes/iwa/ documents pre-BNC fields 3/4 and masks. ~keep
    Ok(match cell_type {
        EMPTY_CELL_TYPE | ERROR_CELL_TYPE => None,
        NUMBER_CELL_TYPE | CURRENCY_CELL_TYPE => fields.double.map(format_scalar),
        TEXT_CELL_TYPE => fields.string_key.and_then(|key| strings.get(&key).cloned()),
        DATE_CELL_TYPE => fields.seconds.map(format_iwork_date),
        BOOLEAN_CELL_TYPE => fields.double.map(|value| (value > 0.0).to_string()),
        DURATION_CELL_TYPE => fields.double.map(|value| format!("{}s", format_scalar(value))),
        RICH_TEXT_CELL_TYPE => fields.rich_key.and_then(|key| rich_strings.get(&key).cloned()),
        _ => None,
    })
}

/// Record that a legacy-format cell carries a formula and/or comment that
/// xberg's wire-level cell parser does not decode (#110). One warning per
/// table per kind: `push_warning` dedupes identical `(source, message)`
/// pairs, so a table with many such cells still surfaces a single line.
fn push_legacy_formula_comment_warning(
    warnings: &mut Vec<ProcessingWarning>,
    table_name: &str,
    fields: &OldCellFields,
) {
    if fields.has_formula {
        crate::core::diagnostics::push_warning(
            warnings,
            crate::extractors::iwork::IWORK_WARNING_SOURCE,
            format!(
                "Table '{table_name}' has a cell with a legacy-format formula; xberg extracts the cell's cached \
                 value but does not reconstruct the formula source text"
            ),
        );
    }
    if fields.has_comment {
        crate::core::diagnostics::push_warning(
            warnings,
            crate::extractors::iwork::IWORK_WARNING_SOURCE,
            format!(
                "Table '{table_name}' has a cell with a legacy-format comment; xberg does not extract cell comment text"
            ),
        );
    }
}

#[derive(Default)]
struct OldCellFields {
    string_key: Option<i32>,
    rich_key: Option<i32>,
    double: Option<f64>,
    seconds: Option<f64>,
    has_formula: bool,
    has_comment: bool,
}

fn parse_old_cell_fields(storage: &[u8], mut cursor: usize, flags: u32) -> Result<OldCellFields> {
    const FIELD_LAYOUT: &[(u32, usize)] = &[
        (OLD_CELL_STYLE_FLAG, STRING_KEY_LENGTH),
        (OLD_TEXT_STYLE_FLAG, STRING_KEY_LENGTH),
        (OLD_CONDITIONAL_STYLE_FLAG, STRING_KEY_LENGTH),
        (OLD_CONDITIONAL_RULE_FLAG, STRING_KEY_LENGTH),
        (OLD_CURRENT_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_FORMULA_FLAG, STRING_KEY_LENGTH),
        (OLD_FORMULA_ERROR_FLAG, STRING_KEY_LENGTH),
        (OLD_RICH_TEXT_FLAG, STRING_KEY_LENGTH),
        (OLD_COMMENT_FLAG, STRING_KEY_LENGTH),
        (OLD_IMPORT_WARNING_FLAG, STRING_KEY_LENGTH),
        (OLD_STRING_FLAG, STRING_KEY_LENGTH),
        (OLD_DOUBLE_FLAG, SCALAR_VALUE_LENGTH),
        (OLD_DATE_FLAG, SCALAR_VALUE_LENGTH),
        (OLD_NUMBER_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_CURRENCY_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_DATE_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_DURATION_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_CONTROL_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_CUSTOM_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_BASE_FORMAT_FLAG, STRING_KEY_LENGTH),
        (OLD_CHOICE_FORMAT_FLAG, STRING_KEY_LENGTH),
    ];
    let mut fields = OldCellFields::default();
    for &(flag, length) in FIELD_LAYOUT {
        let Some(bytes) = take_flagged(storage, &mut cursor, flags, flag, length)? else {
            continue;
        };
        match flag {
            OLD_STRING_FLAG => fields.string_key = Some(decode_i32(bytes)),
            OLD_RICH_TEXT_FLAG => fields.rich_key = Some(decode_i32(bytes)),
            OLD_DOUBLE_FLAG => fields.double = Some(decode_f64(bytes)),
            OLD_DATE_FLAG => fields.seconds = Some(decode_f64(bytes)),
            OLD_FORMULA_FLAG => fields.has_formula = true,
            OLD_COMMENT_FLAG => fields.has_comment = true,
            _ => {}
        }
    }
    Ok(fields)
}

fn take_flagged<'a>(
    storage: &'a [u8],
    cursor: &mut usize,
    flags: u32,
    flag: u32,
    length: usize,
) -> Result<Option<&'a [u8]>> {
    if flags & flag == 0 {
        return Ok(None);
    }
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| numbers_parse_error("Numbers cell field offset overflow"))?;
    let value = storage
        .get(*cursor..end)
        .ok_or_else(|| numbers_parse_error("Numbers cell field is truncated"))?;
    *cursor = end;
    Ok(Some(value))
}

fn decode_i32(bytes: &[u8]) -> i32 {
    let mut value = [0_u8; 4];
    value.copy_from_slice(bytes);
    i32::from_le_bytes(value)
}

fn decode_f64(bytes: &[u8]) -> f64 {
    let mut value = [0_u8; 8];
    value.copy_from_slice(bytes);
    f64::from_le_bytes(value)
}

fn decode_decimal128(bytes: &[u8]) -> f64 {
    let mut mantissa_bytes = [0_u8; 16];
    mantissa_bytes[..14].copy_from_slice(&bytes[..14]);
    mantissa_bytes[14] = bytes[14] & 1;
    let mantissa = u128::from_le_bytes(mantissa_bytes) as f64;
    let exponent = (i32::from(bytes[15] & 0x7f) << 7) | i32::from(bytes[14] >> 1);
    let sign = if bytes[15] & 0x80 == 0 { 1.0 } else { -1.0 };
    sign * mantissa * 10_f64.powi(exponent - DECIMAL128_EXPONENT_BIAS)
}

fn format_scalar(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

fn format_iwork_date(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < i64::MIN as f64 || seconds > i64::MAX as f64 {
        return format_scalar(seconds);
    }
    let unix_seconds = (seconds as i64).saturating_add(IWORK_EPOCH_TO_UNIX_SECONDS);
    let days = unix_seconds.div_euclid(SECONDS_PER_DAY);
    let day_seconds = unix_seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_date_from_unix_days(days);
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_date_from_unix_days(days: i64) -> (i64, i64, i64) {
    const CIVIL_EPOCH_OFFSET_DAYS: i64 = 719_468;
    let adjusted = days + CIVIL_EPOCH_OFFSET_DAYS;
    let era = adjusted.div_euclid(146_097);
    let day_of_era = adjusted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn parse_proto_fields<'a>(data: &'a [u8], budget: &mut SecurityBudget) -> Result<Vec<ProtoField<'a>>> {
    budget.enter()?;
    let result = parse_proto_fields_inner(data, budget);
    budget.leave();
    result
}

fn parse_proto_fields_inner<'a>(data: &'a [u8], budget: &mut SecurityBudget) -> Result<Vec<ProtoField<'a>>> {
    let mut fields = Vec::new();
    let mut position = 0usize;
    while position < data.len() {
        budget.step()?;
        let (tag, tag_length) = read_varint_at(data, position)?;
        position = position
            .checked_add(tag_length)
            .ok_or_else(|| numbers_parse_error("protobuf tag offset overflow"))?;
        let number = u32::try_from(tag >> wire::FIELD_NUMBER_SHIFT)
            .map_err(|_| numbers_parse_error("protobuf field number overflow"))?;
        if let Some(value) = parse_proto_value(data, &mut position, tag & wire::TYPE_MASK)? {
            fields.push(ProtoField { number, value });
        }
    }
    Ok(fields)
}

fn parse_proto_value<'a>(data: &'a [u8], position: &mut usize, wire_type: u64) -> Result<Option<ProtoValue<'a>>> {
    match wire_type {
        wire::VARINT => {
            let (value, length) = read_varint_at(data, *position)?;
            *position = position
                .checked_add(length)
                .ok_or_else(|| numbers_parse_error("protobuf varint offset overflow"))?;
            Ok(Some(ProtoValue::Varint(value)))
        }
        wire::FIXED64 => {
            *position = checked_end(*position, wire::FIXED64_LENGTH, data.len(), "protobuf fixed64")?;
            Ok(None)
        }
        wire::LENGTH_DELIMITED => parse_length_delimited_value(data, position).map(Some),
        wire::FIXED32 => {
            *position = checked_end(*position, wire::FIXED32_LENGTH, data.len(), "protobuf fixed32")?;
            Ok(None)
        }
        _ => Err(numbers_parse_error(&format!(
            "unsupported protobuf wire type {wire_type}"
        ))),
    }
}

fn parse_length_delimited_value<'a>(data: &'a [u8], position: &mut usize) -> Result<ProtoValue<'a>> {
    let (length, prefix_length) = read_varint_at(data, *position)?;
    *position = position
        .checked_add(prefix_length)
        .ok_or_else(|| numbers_parse_error("protobuf length offset overflow"))?;
    let end = checked_end(*position, length, data.len(), "protobuf length-delimited field")?;
    let value = ProtoValue::Bytes(&data[*position..end]);
    *position = end;
    Ok(value)
}

fn read_varint_at(data: &[u8], position: usize) -> Result<(u64, usize)> {
    let mut value = 0u64;
    let mut shift = 0u32;
    let mut cursor = position;
    loop {
        let byte = *data
            .get(cursor)
            .ok_or_else(|| numbers_parse_error("truncated protobuf varint"))?;
        cursor += 1;
        if shift == wire::VARINT_MAX_BITS - 1 && byte > 1 {
            return Err(numbers_parse_error("protobuf varint exceeds 64 bits"));
        }
        value |= u64::from(byte & wire::VARINT_VALUE_MASK) << shift;
        if byte & wire::VARINT_CONTINUATION_BIT == 0 {
            return Ok((value, cursor - position));
        }
        shift += wire::VARINT_BITS_PER_BYTE;
        if shift >= wire::VARINT_MAX_BITS {
            return Err(numbers_parse_error("protobuf varint exceeds 64 bits"));
        }
    }
}

fn checked_end(position: usize, length: u64, total: usize, description: &str) -> Result<usize> {
    let length = usize::try_from(length).map_err(|_| numbers_parse_error(&format!("{description} length overflow")))?;
    let end = position
        .checked_add(length)
        .ok_or_else(|| numbers_parse_error(&format!("{description} offset overflow")))?;
    if end > total {
        return Err(numbers_parse_error(&format!("{description} is truncated")));
    }
    Ok(end)
}

fn field_varint(fields: &[ProtoField<'_>], number: u32) -> Option<u64> {
    fields.iter().find_map(|field| match field {
        ProtoField {
            number: field_number,
            value: ProtoValue::Varint(value),
        } if *field_number == number => Some(*value),
        _ => None,
    })
}

fn field_usize(fields: &[ProtoField<'_>], number: u32) -> Option<usize> {
    field_varint(fields, number).and_then(|value| usize::try_from(value).ok())
}

fn field_bytes<'data, 'fields>(
    fields: &'fields [ProtoField<'data>],
    number: u32,
) -> impl Iterator<Item = &'data [u8]> + 'fields
where
    'data: 'fields,
{
    fields.iter().filter_map(move |field| match field {
        ProtoField {
            number: field_number,
            value: ProtoValue::Bytes(value),
        } if *field_number == number => Some(*value),
        _ => None,
    })
}

fn reference_identifier(data: &[u8]) -> Option<u64> {
    let (tag, tag_length) = read_varint_at(data, 0).ok()?;
    if tag != wire::REFERENCE_TAG {
        return None;
    }
    read_varint_at(data, tag_length).ok().map(|(value, _)| value)
}

fn numbers_parse_error(message: &str) -> XbergError {
    XbergError::parsing(format!("Failed to parse Numbers table data: {message}"))
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for NumbersExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let data = {
            #[cfg(feature = "tokio-runtime")]
            if crate::core::batch_mode::is_batch_mode() {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                let content_owned = content.to_vec();
                let limits = config.security_limits.clone().unwrap_or_default();
                let span = tracing::Span::current();
                tokio::task::spawn_blocking(move || {
                    let _guard = span.entered();
                    parse_numbers(&content_owned, &limits)
                })
                .await
                .map_err(|e| crate::error::XbergError::parsing(format!("Numbers extraction task failed: {e}")))??
            } else {
                let limits = config.security_limits.clone().unwrap_or_default();
                parse_numbers(content, &limits)?
            }

            #[cfg(not(feature = "tokio-runtime"))]
            {
                if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
                    return Err(crate::error::XbergError::Cancelled);
                }
                let limits = config.security_limits.clone().unwrap_or_default();
                parse_numbers(content, &limits)?
            }
        };

        let mut doc = build_numbers_internal_document(&data);
        doc.mime_type = mime_type.to_string();
        for warning in data.warnings {
            crate::core::diagnostics::push_warning_deduped(&mut doc.processing_warnings, warning);
        }
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-iwork-numbers-sffnumbers"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

/// Build an `InternalDocument` from extracted Numbers data.
///
/// Outputs cell values as structured tables rather than flat paragraphs,
/// reflecting the spreadsheet nature of the .numbers format.
fn build_numbers_internal_document(data: &NumbersData) -> InternalDocument {
    let mut builder = InternalDocumentBuilder::new("numbers");

    if data.metadata.title.is_some() || data.metadata.authors.is_some() {
        builder.set_metadata(data.metadata.clone());
    }

    // Sheet names are their own heading, separate from the table heading beneath them —
    // matching the xlsx/ods convention (see `extraction/excel.rs`) of headinging a sheet
    // independently of its content rather than folding the sheet name into a table title.
    let mut last_sheet_name: Option<&str> = None;
    for (sheet_name, table_name, cells) in &data.tables {
        if cells.is_empty() {
            continue;
        }

        match sheet_name.as_deref() {
            Some(name) if last_sheet_name != Some(name) => {
                builder.push_heading(1, name, None, None);
                last_sheet_name = Some(name);
            }
            Some(_) => {}
            None => last_sheet_name = None,
        }

        let table_heading_level = if sheet_name.is_some() { 2 } else { 1 };
        builder.push_heading(table_heading_level, table_name, None, None);
        builder.push_table_from_cells(cells, None, None);
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_numbers_extractor_plugin_interface() {
        let extractor = NumbersExtractor::new();
        assert_eq!(extractor.name(), "iwork-numbers-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_numbers_extractor_supported_mime_types() {
        let extractor = NumbersExtractor::new();
        let types = extractor.supported_mime_types();
        assert!(types.contains(&"application/x-iwork-numbers-sffnumbers"));
    }

    #[test]
    fn should_warn_when_sheet_has_non_table_drawables() {
        // #111: the only .numbers fixture has zero non-table drawables (verified: no
        // shape/image/text-box archive types appear in its Document/Sheet objects), so this
        // exercises the byte-level warning path directly rather than a real document. ~keep
        let mut warnings = Vec::new();

        push_non_table_drawable_warning(&mut warnings, "Sheet1", &[42, 7]);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("Sheet1"));
        assert!(warnings[0].message.contains("2 non-table drawable"));
        assert!(warnings[0].message.contains("42, 7"));
    }

    #[test]
    fn should_not_warn_when_sheet_has_no_non_table_drawables() {
        let mut warnings = Vec::new();

        push_non_table_drawable_warning(&mut warnings, "Sheet1", &[]);

        assert!(warnings.is_empty());
    }

    #[test]
    fn should_warn_when_legacy_cell_has_formula_flag() {
        // #110: the fixture corpus has no .numbers file with a real formula cell (verified by
        // inspecting its IWA members), so this exercises the byte-level warning path directly. ~keep
        let mut warnings = Vec::new();
        let fields = OldCellFields {
            has_formula: true,
            ..Default::default()
        };

        push_legacy_formula_comment_warning(&mut warnings, "Sheet1", &fields);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("legacy-format formula"));
        assert!(warnings[0].message.contains("Sheet1"));
    }

    #[test]
    fn should_warn_when_legacy_cell_has_comment_flag() {
        // #110: no real fixture with a cell comment exists; see the formula test above. ~keep
        let mut warnings = Vec::new();
        let fields = OldCellFields {
            has_comment: true,
            ..Default::default()
        };

        push_legacy_formula_comment_warning(&mut warnings, "Sheet1", &fields);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("legacy-format comment"));
    }

    #[test]
    fn should_not_warn_when_legacy_cell_has_neither_formula_nor_comment() {
        let mut warnings = Vec::new();
        let fields = OldCellFields::default();

        push_legacy_formula_comment_warning(&mut warnings, "Sheet1", &fields);

        assert!(warnings.is_empty());
    }

    #[test]
    fn should_reject_varint_that_overflows_u64() {
        let overflowing = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x02];

        assert!(read_varint_at(&overflowing, 0).is_err());
    }

    #[test]
    fn should_accept_maximum_u64_varint() {
        let maximum = [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01];

        assert_eq!(read_varint_at(&maximum, 0).unwrap(), (u64::MAX, maximum.len()));
    }

    #[test]
    fn should_treat_high_bit_cell_offsets_as_valid_unsigned_values() {
        let offsets = [0x00, 0x80, 0xff, 0xff];

        assert_eq!(
            parse_cell_offsets(&offsets, 2, false).unwrap(),
            vec![Some(32_768), None]
        );
        assert_eq!(
            parse_cell_offsets(&offsets, 2, true).unwrap(),
            vec![Some(131_072), None]
        );
    }

    #[test]
    fn should_account_for_repeated_dictionary_string_expansion() {
        let value = "amplify".to_string();
        let strings = HashMap::from([(7, value)]);
        let mut cell = vec![0; CELL_HEADER_LENGTH + STRING_KEY_LENGTH];
        cell[0] = CELL_STORAGE_VERSION;
        cell[1] = TEXT_CELL_TYPE;
        cell[8..12].copy_from_slice(&CELL_STRING_FLAG.to_le_bytes());
        cell[CELL_HEADER_LENGTH..].copy_from_slice(&7_i32.to_le_bytes());
        let storage = [cell.as_slice(), cell.as_slice()].concat();
        let second_offset = u16::try_from(cell.len()).unwrap();
        let offsets = [0_u16.to_le_bytes(), second_offset.to_le_bytes()].concat();
        let limits = SecurityLimits {
            max_content_size: 10,
            ..SecurityLimits::default()
        };
        let mut budget = SecurityBudget::from_limits(&limits);
        let mut row = vec![String::new(), String::new()];
        let rich_strings = HashMap::new();
        let mut warnings = Vec::new();

        assert!(matches!(
            fill_row(
                &mut row,
                &storage,
                &offsets,
                false,
                &mut TableFillContext {
                    strings: &strings,
                    rich_strings: &rich_strings,
                    budget: &mut budget,
                    table_name: "Table 1",
                    warnings: &mut warnings,
                },
            ),
            Err(XbergError::Security { .. })
        ));
    }

    #[test]
    fn should_skip_unknown_cell_without_downgrading_the_table() {
        let strings = HashMap::new();
        let mut cell = vec![0; CELL_HEADER_LENGTH];
        cell[0] = CELL_STORAGE_VERSION;
        cell[1] = u8::MAX;
        let mut warnings = Vec::new();
        let mut budget = SecurityBudget::for_iwork(&SecurityLimits::default());

        assert_eq!(
            parse_cell_value(
                &cell,
                &mut TableFillContext {
                    strings: &strings,
                    rich_strings: &HashMap::new(),
                    budget: &mut budget,
                    table_name: "Table 1",
                    warnings: &mut warnings,
                },
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn should_decode_mixed_v5_and_legacy_cell_values() {
        let strings = HashMap::from([(7, "plain".to_string())]);
        let rich_strings = HashMap::from([(9, "rich text".to_string())]);
        let decimal = decimal128_integer(123);
        let cases = [
            (v5_cell(NUMBER_CELL_TYPE, CELL_DECIMAL_FLAG, &decimal), Some("123")),
            (
                v5_cell(BOOLEAN_CELL_TYPE, CELL_DOUBLE_FLAG, &1_f64.to_le_bytes()),
                Some("true"),
            ),
            (
                v5_cell(DATE_CELL_TYPE, CELL_DATE_FLAG, &0_f64.to_le_bytes()),
                Some("2001-01-01T00:00:00Z"),
            ),
            (
                v5_cell(TEXT_CELL_TYPE, CELL_STRING_FLAG, &7_i32.to_le_bytes()),
                Some("plain"),
            ),
            (
                v5_cell(RICH_TEXT_CELL_TYPE, CELL_RICH_TEXT_FLAG, &9_i32.to_le_bytes()),
                Some("rich text"),
            ),
            (
                legacy_v4_cell(NUMBER_CELL_TYPE, OLD_DOUBLE_FLAG, &12.5_f64.to_le_bytes()),
                Some("12.5"),
            ),
        ];

        let mut warnings = Vec::new();
        let mut budget = SecurityBudget::for_iwork(&SecurityLimits::default());
        for (cell, expected) in cases {
            assert_eq!(
                parse_cell_value(
                    &cell,
                    &mut TableFillContext {
                        strings: &strings,
                        rich_strings: &rich_strings,
                        budget: &mut budget,
                        table_name: "Table 1",
                        warnings: &mut warnings,
                    },
                )
                .unwrap()
                .as_deref(),
                expected
            );
        }
    }

    #[test]
    fn should_read_legacy_pre_bnc_row_fields_three_and_four() {
        let strings = HashMap::from([(7, "legacy text".to_string())]);
        let cell = legacy_v4_cell(TEXT_CELL_TYPE, OLD_STRING_FLAG, &7_i32.to_le_bytes());
        let mut row_info = vec![8, 0, 26];
        row_info.extend(encode_varint(cell.len() as u64));
        row_info.extend(cell);
        row_info.extend([34, 2, 0, 0]);
        let mut tile = vec![42];
        tile.extend(encode_varint(row_info.len() as u64));
        tile.extend(row_info);
        let mut cells = vec![vec![String::new()]];
        let mut budget = SecurityBudget::for_iwork(&SecurityLimits::default());
        let mut warnings = Vec::new();

        fill_tile(
            &tile,
            0,
            &mut cells,
            &mut TableFillContext {
                strings: &strings,
                rich_strings: &HashMap::new(),
                budget: &mut budget,
                table_name: "Table 1",
                warnings: &mut warnings,
            },
        )
        .unwrap();

        assert_eq!(cells, vec![vec!["legacy text".to_string()]]);
    }

    #[test]
    fn should_reject_required_merge_patch_instead_of_returning_stale_object() {
        let objects = HashMap::from([(
            1,
            vec![
                IwaObject {
                    object_type: DOCUMENT_ARCHIVE_TYPE,
                    payload: vec![1],
                    is_merge_patch: false,
                },
                IwaObject {
                    object_type: DOCUMENT_ARCHIVE_TYPE,
                    payload: vec![2],
                    is_merge_patch: true,
                },
            ],
        )]);

        assert_eq!(objects[&1].len(), 2);
        assert!(reject_required_merge_patches(&objects).is_err());
    }

    #[test]
    fn should_preserve_multiple_message_info_payloads_for_one_identifier() {
        let first = [8, 1];
        let second = [8, 2];
        let mut header = vec![8, 42];
        append_message_info(&mut header, TABLE_MODEL_ARCHIVE_TYPE, first.len());
        append_message_info(&mut header, TABLE_MODEL_ARCHIVE_TYPE, second.len());
        let mut segment = encode_varint(header.len() as u64);
        segment.extend_from_slice(&header);
        segment.extend_from_slice(&first);
        segment.extend_from_slice(&second);
        let mut objects = HashMap::new();
        let mut budget = SecurityBudget::for_iwork(&SecurityLimits::default());

        parse_iwa_segments(&segment, &mut budget, &mut objects).unwrap();

        assert_eq!(objects[&42].len(), 2);
        assert_eq!(objects[&42][0].payload, first);
        assert_eq!(objects[&42][1].payload, second);
    }

    #[test]
    fn should_limit_legacy_cells_before_allocating_all_rows() {
        let limits = SecurityLimits {
            max_table_cells: 1,
            ..SecurityLimits::default()
        };
        let mut budget = SecurityBudget::for_iwork(&limits);
        let mut seen = std::collections::HashSet::new();
        let mut cells = Vec::new();

        assert!(matches!(
            append_legacy_cells(
                vec!["first".to_string(), "second".to_string()],
                &mut seen,
                &mut cells,
                &mut budget
            ),
            Err(XbergError::Security { .. })
        ));
        assert_eq!(cells, vec![vec!["first".to_string()]]);
    }

    /// Regression for #107: legacy fallback text collection must keep a
    /// single-character alphanumeric cell instead of discarding it.
    #[test]
    fn append_legacy_cells_keeps_single_character_alphanumeric_text() {
        let mut seen = std::collections::HashSet::new();
        let mut cells = Vec::new();
        let mut budget = SecurityBudget::for_iwork(&SecurityLimits::default());

        append_legacy_cells(vec!["5".to_string()], &mut seen, &mut cells, &mut budget).unwrap();

        assert_eq!(cells, vec![vec!["5".to_string()]]);
    }

    #[test]
    fn append_legacy_cells_drops_non_alphanumeric_noise() {
        let mut seen = std::collections::HashSet::new();
        let mut cells = Vec::new();
        let mut budget = SecurityBudget::for_iwork(&SecurityLimits::default());

        append_legacy_cells(vec!["--".to_string()], &mut seen, &mut cells, &mut budget).unwrap();

        assert!(cells.is_empty());
    }

    /// Regression for #106: a malformed IWA member reached via the structured
    /// parse path must surface a named `ProcessingWarning`, not vanish silently.
    #[test]
    fn read_iwa_objects_warns_on_malformed_member() {
        let mut archive = Vec::new();
        {
            use std::io::Write;
            let cursor = std::io::Cursor::new(&mut archive);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            // Malformed IWA framing: a chunk type byte with no length/payload.
            zip.start_file("Index/Broken.iwa", options).unwrap();
            zip.write_all(&[1, 0, 0]).unwrap();
            zip.finish().unwrap();
        }

        let limits = SecurityLimits::default();
        let mut budget = SecurityBudget::for_iwork(&limits);
        let mut expansion = IwaExpansionBudget::from_limits(&limits);
        let mut warnings = Vec::new();

        let objects = read_iwa_objects(&archive, &mut budget, &mut expansion, &mut warnings).unwrap();

        assert!(objects.is_empty());
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].source, "iwork");
        assert!(
            warnings[0].message.contains("Index/Broken.iwa"),
            "warning must name the failed member: {}",
            warnings[0].message
        );
    }

    #[test]
    fn should_use_fresh_expansion_budget_for_parse_numbers_fallback() {
        let text = b"fresh fallback";
        let mut protobuf = vec![0x1a, text.len() as u8];
        protobuf.extend_from_slice(text);
        let mut iwa = vec![1, protobuf.len() as u8, 0, 0];
        iwa.extend_from_slice(&protobuf);
        let archive = numbers_zip_with_table_iwa(&iwa);
        let limits = SecurityLimits {
            // One attempt fits exactly; reusing its expansion budget would reject the fallback. ~keep
            max_content_size: iwa.len(),
            ..SecurityLimits::default()
        };

        let data = parse_numbers(&archive, &limits).unwrap();

        assert_eq!(data.tables.len(), 1);
        assert_eq!(data.tables[0].0, None);
        assert_eq!(data.tables[0].1, "Sheet Data");
        assert_eq!(data.tables[0].2, vec![vec!["fresh fallback".to_string()]]);
    }

    fn v5_cell(cell_type: u8, flag: u32, value: &[u8]) -> Vec<u8> {
        let mut cell = vec![0; CELL_HEADER_LENGTH];
        cell[0] = CELL_STORAGE_VERSION;
        cell[1] = cell_type;
        cell[8..12].copy_from_slice(&flag.to_le_bytes());
        cell.extend_from_slice(value);
        cell
    }

    fn legacy_v4_cell(cell_type: u8, flag: u32, value: &[u8]) -> Vec<u8> {
        let mut cell = vec![0; CELL_HEADER_LENGTH];
        cell[0] = 4;
        cell[1] = cell_type;
        cell[4..8].copy_from_slice(&flag.to_le_bytes());
        cell.extend_from_slice(value);
        cell
    }

    fn decimal128_integer(value: u8) -> [u8; DECIMAL_VALUE_LENGTH] {
        let mut decimal = [0_u8; DECIMAL_VALUE_LENGTH];
        decimal[0] = value;
        decimal[14] = ((DECIMAL128_EXPONENT_BIAS & 0x7f) << 1) as u8;
        decimal[15] = (DECIMAL128_EXPONENT_BIAS >> 7) as u8;
        decimal
    }

    fn append_message_info(header: &mut Vec<u8>, object_type: u32, payload_length: usize) {
        let mut message = vec![8];
        message.extend(encode_varint(u64::from(object_type)));
        message.push(24);
        message.extend(encode_varint(payload_length as u64));
        header.push(18);
        header.extend(encode_varint(message.len() as u64));
        header.extend(message);
    }

    fn numbers_zip_with_table_iwa(iwa: &[u8]) -> Vec<u8> {
        use std::io::Write;

        let mut archive = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut archive);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("Index/Table-1.iwa", options).unwrap();
            zip.write_all(iwa).unwrap();
            zip.finish().unwrap();
        }
        archive
    }

    fn encode_varint(mut value: u64) -> Vec<u8> {
        let mut encoded = Vec::new();
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            encoded.push(if value == 0 { byte } else { byte | 0x80 });
            if value == 0 {
                return encoded;
            }
        }
    }
}
