//! WordPerfect (.wpd) extractor.
//!
//! Extracts text content from WordPerfect documents by walking the ordered,
//! properly-nested [`WpdEvent`] stream produced by the vendored `libwpd`
//! shim in `xberg_libwpd`.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::Metadata;
use crate::types::document_structure::{AnnotationKind, ContentLayer, TextAnnotation};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use ahash::AHashMap;
use async_trait::async_trait;
use xberg_libwpd::{WpdDocument, WpdEvent, WpdMetadata};

#[cfg_attr(alef, alef(skip))]
/// Extractor for WordPerfect (.wpd) documents.
///
/// Supports WordPerfect 5.x/6.x (Windows) and Mac WordPerfect documents via
/// the vendored `libwpd`/`librevenge` shim in `xberg_libwpd`.
pub struct WordPerfectExtractor;

impl WordPerfectExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for WordPerfectExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for WordPerfectExtractor {
    fn name(&self) -> &str {
        "wordperfect-extractor"
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
        "WordPerfect (.wpd) text extraction"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

/// The kinds of inline spans tracked while walking the event stream. Carries
/// no payload beyond `Link`'s href; span matching on close compares variant
/// discriminant only (a document is expected to close spans in a way that
/// makes the href irrelevant for matching).
#[derive(Clone)]
enum SpanKind {
    Bold,
    Italic,
    Underline,
    Strikethrough,
    Superscript,
    Subscript,
    Link(String),
}

/// A single open inline span awaiting its matching `*End` event.
struct OpenSpan {
    start: u32,
    kind: SpanKind,
}

/// Accumulates plain text plus in-progress inline spans for one logical unit
/// of content (a paragraph, a table cell, or a diverted body such as a
/// header/footer/note/aside).
#[derive(Default)]
struct Ctx {
    text: String,
    spans: Vec<OpenSpan>,
    annotations: Vec<TextAnnotation>,
}

impl Ctx {
    fn append(&mut self, s: &str) {
        self.text.push_str(s);
    }

    fn open_span(&mut self, kind: SpanKind) {
        let start = self.text.len() as u32;
        self.spans.push(OpenSpan { start, kind });
    }

    /// Record an annotation for `open`, covering the range from where it opened
    /// to the current text end. Empty ranges are dropped.
    fn record_span(&mut self, open: OpenSpan) {
        let end = self.text.len() as u32;
        if end > open.start {
            self.annotations.push(TextAnnotation {
                start: open.start,
                end,
                kind: span_to_annotation(open.kind),
            });
        }
    }

    /// Close the innermost open span matching `kind`'s discriminant, recording
    /// an annotation over the byte range it covered (empty ranges are dropped).
    fn close_span(&mut self, kind: &SpanKind) {
        let target = std::mem::discriminant(kind);
        if let Some(pos) = self
            .spans
            .iter()
            .rposition(|s| std::mem::discriminant(&s.kind) == target)
        {
            let open = self.spans.remove(pos);
            self.record_span(open);
        }
    }

    /// Close every still-open span at the current text end, recording each as an
    /// annotation. Unlike a bare `spans.clear()`, this preserves the formatting
    /// of text accumulated so far when a boundary (paragraph flush, note anchor,
    /// cell/diversion close) arrives before a span's own `*End` event.
    fn close_open_spans(&mut self) {
        for open in std::mem::take(&mut self.spans) {
            self.record_span(open);
        }
    }

    /// Drain this context's text and annotations, merging adjacent same-kind
    /// annotations so consecutive runs of identical formatting don't produce
    /// back-to-back annotation fragments (mirrors the DOCX extractor). Any span
    /// still open at drain time is closed (recorded) rather than discarded.
    fn take(&mut self) -> (String, Vec<TextAnnotation>) {
        self.close_open_spans();
        let text = std::mem::take(&mut self.text);
        let mut annotations = std::mem::take(&mut self.annotations);
        merge_adjacent_annotations(&mut annotations);
        (text, annotations)
    }
}

/// Map an inline span to its `InternalDocument` annotation kind. Built directly
/// rather than via `types::builder`'s annotation helpers: those are gated behind
/// `office`/`email`/`xml`, none of which `wordperfect` implies.
fn span_to_annotation(kind: SpanKind) -> AnnotationKind {
    match kind {
        SpanKind::Bold => AnnotationKind::Bold,
        SpanKind::Italic => AnnotationKind::Italic,
        SpanKind::Underline => AnnotationKind::Underline,
        SpanKind::Strikethrough => AnnotationKind::Strikethrough,
        SpanKind::Superscript => AnnotationKind::Superscript,
        SpanKind::Subscript => AnnotationKind::Subscript,
        SpanKind::Link(href) => AnnotationKind::Link { url: href, title: None },
    }
}

/// Upper bound on a table cell's reported column span. libwpd only clamps the
/// span to a minimum of 1; an implausibly large value (from a corrupted
/// table-definition field) would otherwise drive an unbounded clone loop in
/// `end_cell`. No real table spans more columns than this.
const MAX_TABLE_COL_SPAN: u32 = 4096;

/// Why a diverted body (header/footer/note/aside) was opened, and what to do
/// with its accumulated text when the matching `*End` event closes it.
enum DiversionKind {
    Header,
    Footer,
    /// `key` is the footnote/endnote anchor key shared with the `FootnoteRef`
    /// already pushed at the anchor point.
    Note {
        key: String,
    },
    /// A footnote/endnote anchored inside a table cell or another diversion,
    /// where a standalone `FootnoteRef`/`FootnoteDefinition` element would land
    /// out of reading order (cells and diverted bodies are pushed to the builder
    /// as deferred single units). Its body is folded back inline into the
    /// surrounding text when it closes.
    InlineNote,
    /// `kind` is librevenge's aside kind, `"comment"` or `"box"`.
    Aside {
        kind: String,
    },
}

struct Diversion {
    kind: DiversionKind,
    ctx: Ctx,
}

/// Accumulates rows/cells for a table currently being walked. Column
/// placement is purely positional (sequential push per row), matching the
/// DOCX extractor's approach rather than trusting `CellStart.column`, which
/// libwpd may report as `-1`. `CoveredCell` positions (merged-away grid
/// slots from row/column spans) are filled in from the same column position
/// in the previous row when available, so a vertically merged cell's content
/// still appears in every row it visually covers.
#[derive(Default)]
struct TableBuilder {
    rows: Vec<Vec<String>>,
    current_row: Vec<String>,
    current_row_is_header: bool,
    header_row_indices: Vec<usize>,
    in_cell: bool,
    cell_ctx: Ctx,
    cell_col_span: u32,
}

impl TableBuilder {
    fn start_row(&mut self, header: bool) {
        self.current_row = Vec::new();
        self.current_row_is_header = header;
    }

    fn start_cell(&mut self, col_span: u32) {
        self.in_cell = true;
        self.cell_ctx = Ctx::default();
        self.cell_col_span = col_span.clamp(1, MAX_TABLE_COL_SPAN);
    }

    fn end_cell(&mut self) {
        let (text, annotations) = self.cell_ctx.take();
        // Bake inline formatting into the cell text as markdown (mirrors the
        // DOCX extractor, which renders cell runs to markdown) rather than
        // dropping it: `push_table_from_cells` takes flat strings.
        let mut rendered = annotations_to_markdown(&text, &annotations);
        for i in 0..self.cell_col_span {
            if i + 1 == self.cell_col_span {
                self.current_row.push(std::mem::take(&mut rendered));
            } else {
                self.current_row.push(rendered.clone());
            }
        }
        self.in_cell = false;
    }

    fn covered_cell(&mut self) {
        let col_idx = self.current_row.len();
        let filler = self
            .rows
            .last()
            .and_then(|r| r.get(col_idx))
            .cloned()
            .unwrap_or_default();
        self.current_row.push(filler);
    }

    fn end_row(&mut self) {
        let row = std::mem::take(&mut self.current_row);
        if self.current_row_is_header {
            self.header_row_indices.push(self.rows.len());
        }
        self.rows.push(row);
    }
}

/// Walks a `WpdDocument`'s event stream and pushes elements into an
/// `InternalDocumentBuilder`. See the module doc for the overall mapping.
struct WpdWalker<'a> {
    builder: &'a mut InternalDocumentBuilder,
    main: Ctx,
    pending_heading_level: Option<u8>,
    /// Ordered flag per currently open `push_list()` container, indexed by
    /// nesting depth - 1.
    list_stack: Vec<bool>,
    /// Set while inside a `ListItemStart..ListItemEnd` bracket, so a
    /// `ParagraphEnd` inside it flushes via `push_list_item` instead of
    /// `push_paragraph`.
    current_list_item_ordered: Option<bool>,
    table: Option<TableBuilder>,
    /// Nesting depth of tables opened inside a cell of `table`. WordPerfect
    /// permits cell-nested tables; rather than clobber the outer `TableBuilder`
    /// (losing it entirely), inner table structure is ignored and its text
    /// flows into the enclosing cell. `> 0` means we are inside such an inner
    /// table.
    table_depth: u32,
    diversions: Vec<Diversion>,
    /// Shared counter for footnote and endnote anchor keys.
    note_counter: u32,
}

impl<'a> WpdWalker<'a> {
    fn new(builder: &'a mut InternalDocumentBuilder) -> Self {
        Self {
            builder,
            main: Ctx::default(),
            pending_heading_level: None,
            list_stack: Vec::new(),
            current_list_item_ordered: None,
            table: None,
            table_depth: 0,
            diversions: Vec::new(),
            note_counter: 0,
        }
    }

    fn in_cell(&self) -> bool {
        self.table.as_ref().is_some_and(|t| t.in_cell)
    }

    /// The table currently accepting row/cell events, or `None` when no table is
    /// open or we are inside a nested inner table (`table_depth > 0`), whose
    /// structure is folded into the enclosing cell rather than built separately.
    fn active_table(&mut self) -> Option<&mut TableBuilder> {
        if self.table_depth == 0 {
            self.table.as_mut()
        } else {
            None
        }
    }

    /// The context inline text/spans currently route into: a table cell if
    /// one is open, else the innermost open diversion, else the main
    /// document-body paragraph buffer.
    fn active_ctx(&mut self) -> &mut Ctx {
        if self.in_cell() {
            &mut self.table.as_mut().expect("in_cell implies table is Some").cell_ctx
        } else if let Some(diversion) = self.diversions.last_mut() {
            &mut diversion.ctx
        } else {
            &mut self.main
        }
    }

    fn append(&mut self, s: &str) {
        self.active_ctx().append(s);
    }

    fn open_span(&mut self, kind: SpanKind) {
        self.active_ctx().open_span(kind);
    }

    fn close_span(&mut self, kind: &SpanKind) {
        self.active_ctx().close_span(kind);
    }

    /// Handle `ParagraphEnd`. Inside a table cell or a diversion, a paragraph
    /// break is just a line break within the accumulating text (cells and
    /// diverted bodies are pushed as a single unit when they close). At the
    /// top level it flushes the current paragraph/heading/list-item.
    fn on_paragraph_end(&mut self) {
        if self.in_cell() || !self.diversions.is_empty() {
            self.active_ctx().append("\n");
            return;
        }
        self.flush_paragraph();
    }

    fn flush_paragraph(&mut self) {
        let (text, annotations) = self.main.take();
        if text.trim().is_empty() {
            self.pending_heading_level = None;
            return;
        }
        if let Some(level) = self.pending_heading_level.take() {
            let idx = self.builder.push_heading(level, &text, None, None);
            if !annotations.is_empty() {
                self.builder.set_annotations(idx, annotations);
            }
        } else if let Some(ordered) = self.current_list_item_ordered {
            self.builder.push_list_item(&text, ordered, annotations, None, None);
        } else {
            self.builder.push_paragraph(&text, annotations, None, None);
        }
    }

    fn enter_list_item(&mut self, ordered: bool, level: u8) {
        let level = level.max(1) as usize;
        while self.list_stack.len() > level {
            self.builder.end_list();
            self.list_stack.pop();
        }
        if self.list_stack.len() < level {
            while self.list_stack.len() < level {
                self.builder.push_list(ordered);
                self.list_stack.push(ordered);
            }
        } else if self.list_stack.last() != Some(&ordered) {
            self.builder.end_list();
            self.list_stack.pop();
            self.builder.push_list(ordered);
            self.list_stack.push(ordered);
        }
        self.current_list_item_ordered = Some(ordered);
    }

    fn exit_list_item(&mut self) {
        // Always drain `main` (flush_paragraph's own empty-text guard suppresses
        // an empty push): a whitespace-only item that still carries a recorded
        // annotation must not leak its stale text/annotation into the next
        // element.
        self.flush_paragraph();
        self.current_list_item_ordered = None;
    }

    fn enter_note(&mut self, endnote: bool) {
        self.note_counter += 1;
        // A note anchored inside a table cell or another diversion can't become
        // a standalone `FootnoteRef`/`FootnoteDefinition` element without
        // landing out of reading order (cells/diverted bodies are pushed as
        // deferred units). Fold it inline instead: an inline marker here, the
        // body appended back into the surrounding text at `NoteEnd`.
        if !self.diversions.is_empty() || self.in_cell() {
            let marker = format!("[{}]", self.note_counter);
            self.append(&marker);
            self.diversions.push(Diversion {
                kind: DiversionKind::InlineNote,
                ctx: Ctx::default(),
            });
            return;
        }

        // Top level: flush the pre-anchor body so the footnote-ref marker
        // element lands at the right point in reading order, then divert
        // subsequent events into the note body until `NoteEnd`. Footnotes and
        // endnotes share the same InternalDocument mechanism
        // (`FootnoteRef`/`FootnoteDefinition`) since there is no separate
        // endnote element kind; they are distinguished only by the anchor key
        // prefix ("fn" vs "en"), mirroring how the DOCX extractor chains
        // `doc.footnotes` and `doc.endnotes`. Spans still open across the anchor
        // are snapshotted and reopened so formatting continues after the note.
        let open_kinds: Vec<SpanKind> = self.main.spans.iter().map(|s| s.kind.clone()).collect();
        self.flush_paragraph();
        for kind in open_kinds {
            self.main.open_span(kind);
        }
        let prefix = if endnote { "en" } else { "fn" };
        let key = format!("{prefix}{}", self.note_counter);
        let marker = self.note_counter.to_string();
        self.builder.push_footnote_ref(&marker, &key, None);
        self.diversions.push(Diversion {
            kind: DiversionKind::Note { key },
            ctx: Ctx::default(),
        });
    }

    fn enter_diversion(&mut self, kind: DiversionKind) {
        self.diversions.push(Diversion {
            kind,
            ctx: Ctx::default(),
        });
    }

    fn exit_diversion(&mut self) {
        let Some(mut diversion) = self.diversions.pop() else {
            return;
        };
        let (text, annotations) = diversion.ctx.take();
        match diversion.kind {
            DiversionKind::Header => {
                if !text.trim().is_empty() {
                    let idx = self.builder.push_paragraph(&text, annotations, None, None);
                    self.builder.set_layer(idx, ContentLayer::Header);
                }
            }
            DiversionKind::Footer => {
                if !text.trim().is_empty() {
                    let idx = self.builder.push_paragraph(&text, annotations, None, None);
                    self.builder.set_layer(idx, ContentLayer::Footer);
                }
            }
            DiversionKind::Aside { kind } => {
                if !text.trim().is_empty() {
                    let idx = self.builder.push_paragraph(&text, annotations, None, None);
                    let mut attrs = AHashMap::new();
                    attrs.insert("aside_kind".to_string(), kind);
                    self.builder.set_attributes(idx, attrs);
                }
            }
            DiversionKind::Note { key } => {
                self.builder.push_footnote_definition(&text, &key, None);
            }
            DiversionKind::InlineNote => {
                // Fold the note body back into the surrounding text (the parent
                // context, now active again after the pop above). Annotations
                // are dropped here — this is a rare nested-note fallback.
                let body = text.trim();
                if !body.is_empty() {
                    self.active_ctx().append(" ");
                    self.active_ctx().append(body);
                }
            }
        }
    }

    fn walk(&mut self, events: &[WpdEvent]) {
        for event in events {
            match event {
                WpdEvent::Text(s) => self.append(s),
                WpdEvent::Tab => self.append("\t"),
                WpdEvent::Space => self.append(" "),
                WpdEvent::LineBreak => self.append("\n"),
                WpdEvent::Field(s) => self.append(s),
                WpdEvent::ParagraphEnd => self.on_paragraph_end(),
                WpdEvent::HeadingStart { level } => self.pending_heading_level = Some(*level),
                WpdEvent::BoldStart => self.open_span(SpanKind::Bold),
                WpdEvent::BoldEnd => self.close_span(&SpanKind::Bold),
                WpdEvent::ItalicStart => self.open_span(SpanKind::Italic),
                WpdEvent::ItalicEnd => self.close_span(&SpanKind::Italic),
                WpdEvent::UnderlineStart => self.open_span(SpanKind::Underline),
                WpdEvent::UnderlineEnd => self.close_span(&SpanKind::Underline),
                WpdEvent::StrikethroughStart => self.open_span(SpanKind::Strikethrough),
                WpdEvent::StrikethroughEnd => self.close_span(&SpanKind::Strikethrough),
                WpdEvent::SuperscriptStart => self.open_span(SpanKind::Superscript),
                WpdEvent::SuperscriptEnd => self.close_span(&SpanKind::Superscript),
                WpdEvent::SubscriptStart => self.open_span(SpanKind::Subscript),
                WpdEvent::SubscriptEnd => self.close_span(&SpanKind::Subscript),
                WpdEvent::LinkStart { href } => self.open_span(SpanKind::Link(href.clone())),
                WpdEvent::LinkEnd => self.close_span(&SpanKind::Link(String::new())),
                WpdEvent::ListItemStart { ordered, level, .. } => self.enter_list_item(*ordered, *level),
                WpdEvent::ListItemEnd => self.exit_list_item(),
                WpdEvent::TableStart => {
                    if self.table.is_some() {
                        // Table nested inside an open cell: keep the outer table,
                        // let this inner table's text fold into the enclosing cell.
                        self.table_depth += 1;
                    } else {
                        self.table = Some(TableBuilder::default());
                    }
                }
                WpdEvent::RowStart { header } => {
                    let header = *header;
                    if let Some(t) = self.active_table() {
                        t.start_row(header);
                    }
                }
                WpdEvent::CellStart { col_span, .. } => {
                    let col_span = *col_span;
                    if let Some(t) = self.active_table() {
                        t.start_cell(col_span);
                    }
                }
                WpdEvent::CoveredCell { .. } => {
                    if let Some(t) = self.active_table() {
                        t.covered_cell();
                    }
                }
                WpdEvent::CellEnd => {
                    if let Some(t) = self.active_table() {
                        t.end_cell();
                    }
                }
                WpdEvent::RowEnd => {
                    if let Some(t) = self.active_table() {
                        t.end_row();
                    }
                }
                WpdEvent::TableEnd => {
                    if self.table_depth > 0 {
                        self.table_depth -= 1;
                    } else {
                        self.end_table();
                    }
                }
                WpdEvent::HeaderStart => self.enter_diversion(DiversionKind::Header),
                WpdEvent::HeaderEnd => self.exit_diversion(),
                WpdEvent::FooterStart => self.enter_diversion(DiversionKind::Footer),
                WpdEvent::FooterEnd => self.exit_diversion(),
                WpdEvent::NoteStart { endnote } => self.enter_note(*endnote),
                WpdEvent::NoteEnd => self.exit_diversion(),
                WpdEvent::AsideStart { kind } => self.enter_diversion(DiversionKind::Aside { kind: kind.clone() }),
                WpdEvent::AsideEnd => self.exit_diversion(),
            }
        }

        self.flush_paragraph();
        while self.list_stack.pop().is_some() {
            self.builder.end_list();
        }
    }

    fn end_table(&mut self) {
        let Some(t) = self.table.take() else {
            return;
        };
        if t.rows.is_empty() {
            return;
        }
        let idx = self.builder.push_table_from_cells(&t.rows, None, None);
        if !t.header_row_indices.is_empty() {
            let header_rows = t
                .header_row_indices
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",");
            let mut attrs = AHashMap::new();
            attrs.insert("header_rows".to_string(), header_rows);
            self.builder.set_attributes(idx, attrs);
        }
    }
}

/// Merge adjacent or overlapping annotations of the same kind so consecutive
/// spans of identical formatting collapse into one annotation instead of
/// several back-to-back fragments. Mirrors the DOCX extractor's
/// `merge_adjacent_annotations`.
fn merge_adjacent_annotations(annotations: &mut Vec<TextAnnotation>) {
    if annotations.len() < 2 {
        return;
    }

    fn same_kind_for_merge(a: &AnnotationKind, b: &AnnotationKind) -> bool {
        match (a, b) {
            (AnnotationKind::Bold, AnnotationKind::Bold)
            | (AnnotationKind::Italic, AnnotationKind::Italic)
            | (AnnotationKind::Underline, AnnotationKind::Underline)
            | (AnnotationKind::Strikethrough, AnnotationKind::Strikethrough)
            | (AnnotationKind::Subscript, AnnotationKind::Subscript)
            | (AnnotationKind::Superscript, AnnotationKind::Superscript) => true,
            (AnnotationKind::Link { url: url_a, .. }, AnnotationKind::Link { url: url_b, .. }) => url_a == url_b,
            _ => false,
        }
    }

    let kind_key = |kind: &AnnotationKind| -> u8 {
        match kind {
            AnnotationKind::Bold => 0,
            AnnotationKind::Italic => 1,
            AnnotationKind::Underline => 2,
            AnnotationKind::Strikethrough => 3,
            AnnotationKind::Subscript => 4,
            AnnotationKind::Superscript => 5,
            AnnotationKind::Link { .. } => 6,
            _ => 255,
        }
    };

    annotations.sort_by(|a, b| kind_key(&a.kind).cmp(&kind_key(&b.kind)).then(a.start.cmp(&b.start)));

    let mut merged = Vec::with_capacity(annotations.len());
    let mut i = 0;
    while i < annotations.len() {
        let mut ann = annotations[i].clone();
        let mut j = i + 1;
        while j < annotations.len()
            && same_kind_for_merge(&annotations[j].kind, &ann.kind)
            && annotations[j].start <= ann.end
        {
            ann.end = ann.end.max(annotations[j].end);
            j += 1;
        }
        merged.push(ann);
        i = j;
    }

    *annotations = merged;
}

/// The markdown open/close markers for an annotation kind, if it has an inline
/// markdown representation. Superscript/subscript and any non-inline kinds
/// return `None` and are rendered as plain text (they have no portable markdown
/// form inside a table cell).
fn markdown_markers(kind: &AnnotationKind) -> Option<(String, String)> {
    match kind {
        AnnotationKind::Bold => Some(("**".to_string(), "**".to_string())),
        AnnotationKind::Italic => Some(("*".to_string(), "*".to_string())),
        AnnotationKind::Strikethrough => Some(("~~".to_string(), "~~".to_string())),
        AnnotationKind::Link { url, .. } => Some(("[".to_string(), format!("]({url})"))),
        _ => None,
    }
}

/// Render `text` with its inline annotations baked in as markdown, for contexts
/// (table cells) that store flat strings rather than a separate annotation list.
/// Annotation ranges use byte offsets that always fall on `char` boundaries
/// (spans open/close only at append boundaries). Half-open `[start, end)`
/// ranges: at each boundary, closings for ranges ending there are emitted before
/// openings for ranges starting there.
fn annotations_to_markdown(text: &str, annotations: &[TextAnnotation]) -> String {
    if annotations.is_empty() {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len() + annotations.len() * 4);
    let emit_boundary = |result: &mut String, pos: u32| {
        for a in annotations {
            if a.end == pos
                && let Some((_, close)) = markdown_markers(&a.kind)
            {
                result.push_str(&close);
            }
        }
        for a in annotations {
            if a.start == pos
                && let Some((open, _)) = markdown_markers(&a.kind)
            {
                result.push_str(&open);
            }
        }
    };
    let mut end_pos = 0u32;
    for (byte, ch) in text.char_indices() {
        emit_boundary(&mut result, byte as u32);
        result.push(ch);
        end_pos = byte as u32 + ch.len_utf8() as u32;
    }
    emit_boundary(&mut result, end_pos);
    result
}

fn apply_metadata(builder: &mut InternalDocumentBuilder, meta: &WpdMetadata) {
    builder.set_metadata(Metadata {
        title: meta.title.clone(),
        subject: meta.subject.clone(),
        authors: meta.author.clone().map(|a| vec![a]),
        keywords: meta.keywords.clone().map(|k| vec![k]),
        ..Metadata::default()
    });
}

fn build_wordperfect_internal_document(doc: &WpdDocument) -> InternalDocument {
    let mut builder = InternalDocumentBuilder::new("wordperfect");
    apply_metadata(&mut builder, &doc.metadata);
    WpdWalker::new(&mut builder).walk(&doc.events);
    builder.build()
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for WordPerfectExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let limits = config.security_limits.clone().unwrap_or_default();
        if content.len() > limits.max_content_size {
            return Err(crate::XbergError::validation(format!(
                "WordPerfect file exceeds size limit ({} > {} bytes)",
                content.len(),
                limits.max_content_size
            )));
        }
        if config.cancel_token.as_ref().map(|t| t.is_cancelled()).unwrap_or(false) {
            return Err(crate::XbergError::Cancelled);
        }

        // libwpd is a synchronous, non-yielding native C++ parser: run it on a
        // blocking thread so the async runtime's `extraction_timeout_secs`
        // timeout and worker-pool concurrency are not defeated by a slow or
        // pathological document (mirrors `doc.rs`/`docx.rs`).
        let doc = {
            #[cfg(feature = "tokio-runtime")]
            {
                let content_owned = content.to_vec();
                let span = tracing::Span::current();
                tokio::task::spawn_blocking(move || {
                    let _guard = span.entered();
                    xberg_libwpd::extract_document(&content_owned)
                })
                .await
                .map_err(|e| crate::XbergError::parsing(format!("WordPerfect extraction task failed: {e}")))?
            }
            #[cfg(not(feature = "tokio-runtime"))]
            {
                xberg_libwpd::extract_document(content)
            }
        }
        .map_err(|e| crate::XbergError::parsing(format!("Failed to read WordPerfect document: {e}")))?;

        let mut internal_doc = build_wordperfect_internal_document(&doc);
        if internal_doc.elements.is_empty() {
            return Err(crate::XbergError::parsing(
                "WordPerfect document produced no extractable content".to_string(),
            ));
        }
        internal_doc.mime_type = mime_type.to_string();
        Ok(internal_doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/vnd.wordperfect", "application/wordperfect"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wordperfect_extractor_plugin_interface() {
        let extractor = WordPerfectExtractor::new();
        assert_eq!(extractor.name(), "wordperfect-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert_eq!(
            extractor.supported_mime_types(),
            &["application/vnd.wordperfect", "application/wordperfect"]
        );
    }

    #[test]
    fn test_wordperfect_extractor_initialize_shutdown() {
        let extractor = WordPerfectExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_wordperfect_extractor_default() {
        let extractor = WordPerfectExtractor;
        assert_eq!(extractor.name(), "wordperfect-extractor");
    }

    #[test]
    fn test_build_document_simple_paragraph_with_bold() {
        let doc = WpdDocument {
            events: vec![
                WpdEvent::BoldStart,
                WpdEvent::Text("Hello".to_string()),
                WpdEvent::BoldEnd,
                WpdEvent::Space,
                WpdEvent::Text("world".to_string()),
                WpdEvent::ParagraphEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        assert_eq!(internal_doc.elements.len(), 1);
        let elem = &internal_doc.elements[0];
        assert_eq!(elem.text, "Hello world");
        assert_eq!(elem.kind, crate::types::internal::ElementKind::Paragraph);
        assert_eq!(elem.annotations.len(), 1);
        assert_eq!(elem.annotations[0].kind, AnnotationKind::Bold);
        assert_eq!(elem.annotations[0].start, 0);
        assert_eq!(elem.annotations[0].end, 5);
    }

    #[test]
    fn test_build_document_heading() {
        let doc = WpdDocument {
            events: vec![
                WpdEvent::HeadingStart { level: 2 },
                WpdEvent::Text("Title".to_string()),
                WpdEvent::ParagraphEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        assert_eq!(internal_doc.elements.len(), 1);
        assert_eq!(
            internal_doc.elements[0].kind,
            crate::types::internal::ElementKind::Heading { level: 2 }
        );
        assert_eq!(internal_doc.elements[0].text, "Title");
    }

    #[test]
    fn test_build_document_list() {
        let doc = WpdDocument {
            events: vec![
                WpdEvent::ListItemStart {
                    ordered: true,
                    level: 1,
                    counter: 1,
                },
                WpdEvent::Text("First".to_string()),
                WpdEvent::ParagraphEnd,
                WpdEvent::ListItemEnd,
                WpdEvent::ListItemStart {
                    ordered: true,
                    level: 1,
                    counter: 2,
                },
                WpdEvent::Text("Second".to_string()),
                WpdEvent::ParagraphEnd,
                WpdEvent::ListItemEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        use crate::types::internal::ElementKind;
        assert_eq!(internal_doc.elements[0].kind, ElementKind::ListStart { ordered: true });
        assert_eq!(internal_doc.elements[1].kind, ElementKind::ListItem { ordered: true });
        assert_eq!(internal_doc.elements[1].text, "First");
        assert_eq!(internal_doc.elements[2].kind, ElementKind::ListItem { ordered: true });
        assert_eq!(internal_doc.elements[2].text, "Second");
        assert_eq!(internal_doc.elements[3].kind, ElementKind::ListEnd);
    }

    #[test]
    fn test_build_document_table() {
        let doc = WpdDocument {
            events: vec![
                WpdEvent::TableStart,
                WpdEvent::RowStart { header: true },
                WpdEvent::CellStart {
                    column: 0,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::Text("A".to_string()),
                WpdEvent::CellEnd,
                WpdEvent::CellStart {
                    column: 1,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::Text("B".to_string()),
                WpdEvent::CellEnd,
                WpdEvent::RowEnd,
                WpdEvent::RowStart { header: false },
                WpdEvent::CellStart {
                    column: 0,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::Text("1".to_string()),
                WpdEvent::CellEnd,
                WpdEvent::CellStart {
                    column: 1,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::Text("2".to_string()),
                WpdEvent::CellEnd,
                WpdEvent::RowEnd,
                WpdEvent::TableEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        use crate::types::internal::ElementKind;
        assert_eq!(internal_doc.elements.len(), 1);
        assert!(matches!(internal_doc.elements[0].kind, ElementKind::Table { .. }));
        assert_eq!(internal_doc.tables.len(), 1);
        assert_eq!(
            internal_doc.tables[0].cells,
            vec![
                vec!["A".to_string(), "B".to_string()],
                vec!["1".to_string(), "2".to_string()],
            ]
        );
        let attrs = internal_doc.elements[0].attributes.as_ref().unwrap();
        assert_eq!(attrs.get("header_rows").unwrap(), "0");
    }

    #[test]
    fn test_build_document_footnote() {
        let doc = WpdDocument {
            events: vec![
                WpdEvent::Text("See".to_string()),
                WpdEvent::NoteStart { endnote: false },
                WpdEvent::Text("A footnote body.".to_string()),
                WpdEvent::NoteEnd,
                WpdEvent::Text("here.".to_string()),
                WpdEvent::ParagraphEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        use crate::types::internal::ElementKind;

        let ref_elem = internal_doc
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::FootnoteRef)
            .expect("expected a FootnoteRef element");
        assert_eq!(ref_elem.anchor.as_deref(), Some("fn1"));

        let def_elem = internal_doc
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::FootnoteDefinition)
            .expect("expected a FootnoteDefinition element");
        assert_eq!(def_elem.text, "A footnote body.");
        assert_eq!(def_elem.anchor.as_deref(), Some("fn1"));

        let trailing = internal_doc
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::Paragraph && e.text == "here.");
        assert!(trailing.is_some(), "text after NoteEnd should form its own paragraph");
    }

    #[test]
    fn test_build_document_header_and_footer_are_bracketed() {
        let doc = WpdDocument {
            events: vec![
                WpdEvent::HeaderStart,
                WpdEvent::Text("Running Header".to_string()),
                WpdEvent::HeaderEnd,
                WpdEvent::Text("Body text.".to_string()),
                WpdEvent::ParagraphEnd,
                WpdEvent::FooterStart,
                WpdEvent::Text("Running Footer".to_string()),
                WpdEvent::FooterEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);

        let header = internal_doc
            .elements
            .iter()
            .find(|e| e.text == "Running Header")
            .unwrap();
        assert_eq!(header.layer, ContentLayer::Header);

        let footer = internal_doc
            .elements
            .iter()
            .find(|e| e.text == "Running Footer")
            .unwrap();
        assert_eq!(footer.layer, ContentLayer::Footer);

        let body = internal_doc.elements.iter().find(|e| e.text == "Body text.").unwrap();
        assert_eq!(body.layer, ContentLayer::Body);
    }

    #[test]
    fn test_build_document_aside_tagged_and_kept_separate() {
        let doc = WpdDocument {
            events: vec![
                WpdEvent::Text("Main text.".to_string()),
                WpdEvent::AsideStart {
                    kind: "comment".to_string(),
                },
                WpdEvent::Text("A reviewer comment.".to_string()),
                WpdEvent::AsideEnd,
                WpdEvent::ParagraphEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);

        let aside = internal_doc
            .elements
            .iter()
            .find(|e| e.text == "A reviewer comment.")
            .expect("aside body pushed as its own element");
        assert_eq!(aside.attributes.as_ref().unwrap().get("aside_kind").unwrap(), "comment");

        let main = internal_doc
            .elements
            .iter()
            .find(|e| e.text == "Main text.")
            .expect("main text not spliced with aside content");
        assert_ne!(main.text, "Main text.A reviewer comment.");
    }

    #[test]
    fn test_build_document_metadata() {
        let doc = WpdDocument {
            events: vec![],
            metadata: WpdMetadata {
                title: Some("My Doc".to_string()),
                author: Some("Jane Doe".to_string()),
                subject: Some("A subject".to_string()),
                keywords: Some("k1, k2".to_string()),
                raw: vec![],
            },
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        assert_eq!(internal_doc.metadata.title.as_deref(), Some("My Doc"));
        assert_eq!(internal_doc.metadata.authors, Some(vec!["Jane Doe".to_string()]));
        assert_eq!(internal_doc.metadata.subject.as_deref(), Some("A subject"));
        assert_eq!(internal_doc.metadata.keywords, Some(vec!["k1, k2".to_string()]));
    }

    #[test]
    fn test_formatting_spanning_a_footnote_anchor_is_preserved() {
        // Regression: a bold run bracketing a footnote anchor must stay bold on
        // both the pre-anchor and post-anchor text (the note flush must not drop
        // the still-open span).
        let doc = WpdDocument {
            events: vec![
                WpdEvent::BoldStart,
                WpdEvent::Text("before".to_string()),
                WpdEvent::NoteStart { endnote: false },
                WpdEvent::Text("note body".to_string()),
                WpdEvent::NoteEnd,
                WpdEvent::Text("after".to_string()),
                WpdEvent::BoldEnd,
                WpdEvent::ParagraphEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        let before = internal_doc
            .elements
            .iter()
            .find(|e| e.text == "before")
            .expect("pre-anchor paragraph");
        assert_eq!(before.annotations.len(), 1);
        assert_eq!(before.annotations[0].kind, AnnotationKind::Bold);
        assert_eq!((before.annotations[0].start, before.annotations[0].end), (0, 6));

        let after = internal_doc
            .elements
            .iter()
            .find(|e| e.text == "after")
            .expect("post-anchor paragraph");
        assert_eq!(after.annotations.len(), 1, "formatting must continue after the note");
        assert_eq!(after.annotations[0].kind, AnnotationKind::Bold);
        assert_eq!((after.annotations[0].start, after.annotations[0].end), (0, 5));
    }

    #[test]
    fn test_table_cell_inline_formatting_rendered_as_markdown() {
        // Regression: bold/italic/link inside a cell were dropped entirely.
        let doc = WpdDocument {
            events: vec![
                WpdEvent::TableStart,
                WpdEvent::RowStart { header: false },
                WpdEvent::CellStart {
                    column: 0,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::BoldStart,
                WpdEvent::Text("Hi".to_string()),
                WpdEvent::BoldEnd,
                WpdEvent::CellEnd,
                WpdEvent::RowEnd,
                WpdEvent::TableEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        assert_eq!(internal_doc.tables.len(), 1);
        assert_eq!(internal_doc.tables[0].cells, vec![vec!["**Hi**".to_string()]]);
    }

    #[test]
    fn test_whitespace_list_item_does_not_leak_into_next_element() {
        // Regression: a whitespace-only list item carrying a closed annotation
        // must not leave stale text/annotation on `main` that leaks forward.
        let doc = WpdDocument {
            events: vec![
                WpdEvent::ListItemStart {
                    ordered: true,
                    level: 1,
                    counter: 1,
                },
                WpdEvent::BoldStart,
                WpdEvent::Text(" ".to_string()),
                WpdEvent::BoldEnd,
                WpdEvent::ListItemEnd,
                WpdEvent::Text("Hello".to_string()),
                WpdEvent::ParagraphEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        let hello = internal_doc
            .elements
            .iter()
            .find(|e| e.text.trim() == "Hello")
            .expect("Hello element");
        assert_eq!(hello.text, "Hello", "leading whitespace must not leak in");
        assert!(hello.annotations.is_empty(), "stray annotation must not leak in");
    }

    #[test]
    fn test_nested_table_does_not_destroy_outer_table() {
        // Regression: an inner table nested in a cell clobbered the outer
        // TableBuilder; the outer table must survive and the inner text folds
        // into the enclosing cell.
        let doc = WpdDocument {
            events: vec![
                WpdEvent::TableStart,
                WpdEvent::RowStart { header: false },
                WpdEvent::CellStart {
                    column: 0,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::Text("outer".to_string()),
                WpdEvent::TableStart,
                WpdEvent::RowStart { header: false },
                WpdEvent::CellStart {
                    column: 0,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::Text("inner".to_string()),
                WpdEvent::CellEnd,
                WpdEvent::RowEnd,
                WpdEvent::TableEnd,
                WpdEvent::CellEnd,
                WpdEvent::RowEnd,
                WpdEvent::TableEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        assert_eq!(internal_doc.tables.len(), 1, "outer table must not be lost");
        assert!(internal_doc.tables[0].cells[0][0].contains("outer"));
    }

    #[test]
    fn test_footnote_inside_table_cell_does_not_reorder_before_table() {
        // Regression: a note anchored in a cell pushed a standalone FootnoteRef
        // ahead of the deferred Table element. It must fold inline instead.
        use crate::types::internal::ElementKind;
        let doc = WpdDocument {
            events: vec![
                WpdEvent::TableStart,
                WpdEvent::RowStart { header: false },
                WpdEvent::CellStart {
                    column: 0,
                    col_span: 1,
                    row_span: 1,
                },
                WpdEvent::Text("cell".to_string()),
                WpdEvent::NoteStart { endnote: false },
                WpdEvent::Text("note".to_string()),
                WpdEvent::NoteEnd,
                WpdEvent::CellEnd,
                WpdEvent::RowEnd,
                WpdEvent::TableEnd,
            ],
            metadata: WpdMetadata::default(),
        };

        let internal_doc = build_wordperfect_internal_document(&doc);
        assert_eq!(internal_doc.tables.len(), 1);
        assert!(
            !internal_doc.elements.iter().any(|e| e.kind == ElementKind::FootnoteRef),
            "nested note must not emit a standalone FootnoteRef"
        );
        assert!(
            internal_doc.tables[0].cells[0][0].contains("[1]"),
            "inline marker folded into cell"
        );
    }

    #[tokio::test]
    async fn test_extract_content_empty_document_errors() {
        use crate::core::config::ExtractionConfig;

        let extractor = WordPerfectExtractor::new();
        let config = ExtractionConfig::default();
        // Not a valid WordPerfect document; extract_document should fail fast
        // rather than panicking on malformed/empty input.
        let result = extractor
            .extract_content(b"not a wordperfect document", "application/vnd.wordperfect", &config)
            .await;
        assert!(result.is_err());
    }
}
