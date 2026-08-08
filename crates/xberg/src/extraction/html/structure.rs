//! HTML to `DocumentStructure` builder.
//!
//! Walks raw HTML and produces a hierarchical `DocumentStructure` using the
//! `DocumentStructureBuilder`. This is intentionally a lightweight, non-allocating
//! tag-level parser that handles the common structural elements without pulling
//! in a full DOM library.

use ahash::AHashMap;

use crate::types::builder::{self, DocumentStructureBuilder};
use crate::types::document_structure::{DocumentStructure, NodeIndex, TextAnnotation};

/// Build a `DocumentStructure` from raw HTML.
pub(crate) fn build_document_structure(html: &str) -> DocumentStructure {
    let mut builder = DocumentStructureBuilder::new().source_format("html");
    let mut walker = HtmlWalker::new(html, &mut builder);
    walker.walk();
    builder.build()
}

/// Tracks the kind of inline formatting active at a given byte offset.
#[derive(Debug, Clone)]
struct InlineSpan {
    kind: InlineKind,
    /// Byte offset in the accumulated text buffer where this span starts.
    text_start: u32,
}

#[derive(Debug, Clone)]
enum InlineKind {
    Bold,
    Italic,
    Code,
    Underline,
    Strikethrough,
    Link { href: String, title: Option<String> },
    Subscript,
    Superscript,
    Highlight,
}

/// Represents a `<pre><code>` block being accumulated.
#[derive(Debug)]
struct PreBlock {
    language: Option<String>,
    text: String,
}

/// Metadata about a single cell being accumulated.
#[derive(Debug)]
struct CellMeta {
    text: String,
    col_span: u32,
    row_span: u32,
    is_header: bool,
}

/// Represents a `<table>` being accumulated.
#[derive(Debug)]
struct TableAccumulator {
    rows: Vec<Vec<CellMeta>>,
    current_row: Vec<CellMeta>,
    current_cell: String,
    current_col_span: u32,
    current_row_span: u32,
    current_is_header: bool,
    in_row: bool,
    in_cell: bool,
}

impl TableAccumulator {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            current_row: Vec::new(),
            current_cell: String::new(),
            current_col_span: 1,
            current_row_span: 1,
            current_is_header: false,
            in_row: false,
            in_cell: false,
        }
    }

    fn open_row(&mut self) {
        self.current_row = Vec::new();
        self.in_row = true;
    }

    fn close_row(&mut self) {
        if self.in_row {
            self.rows.push(std::mem::take(&mut self.current_row));
            self.in_row = false;
        }
    }

    fn open_cell(&mut self, col_span: u32, row_span: u32, is_header: bool) {
        self.current_cell = String::new();
        self.current_col_span = col_span;
        self.current_row_span = row_span;
        self.current_is_header = is_header;
        self.in_cell = true;
    }

    fn close_cell(&mut self) {
        if self.in_cell {
            self.current_row.push(CellMeta {
                text: std::mem::take(&mut self.current_cell),
                col_span: self.current_col_span,
                row_span: self.current_row_span,
                is_header: self.current_is_header,
            });
            self.in_cell = false;
            self.current_col_span = 1;
            self.current_row_span = 1;
            self.current_is_header = false;
        }
    }

    fn push_text(&mut self, text: &str) {
        if self.in_cell {
            self.current_cell.push_str(text);
        }
    }
}

/// List context pushed onto the list stack.
#[derive(Debug)]
struct ListContext {
    node_idx: NodeIndex,
}

/// Definition list context.
#[derive(Debug)]
struct DefListContext {
    list_idx: NodeIndex,
    current_term: Option<String>,
}

/// Figure context accumulating image + caption.
#[derive(Debug)]
struct FigureContext {
    img_alt: Option<String>,
    img_src: Option<String>,
    img_width: Option<String>,
    img_height: Option<String>,
    caption: Option<String>,
    in_caption: bool,
}

/// Main walker state.
struct HtmlWalker<'a, 'b> {
    src: &'a str,
    pos: usize,
    builder: &'b mut DocumentStructureBuilder,

    text_buf: String,
    inline_stack: Vec<InlineSpan>,
    annotations: Vec<TextAnnotation>,

    in_pre: bool,
    pre_block: Option<PreBlock>,
    table: Option<TableAccumulator>,
    list_stack: Vec<ListContext>,
    in_list_item: bool,
    list_item_text: String,
    def_list: Option<DefListContext>,
    in_dt: bool,
    in_dd: bool,
    dt_text: String,
    dd_text: String,
    figure: Option<FigureContext>,
    in_head: bool,
    meta_entries: Vec<(String, String)>,

    pending_classes: Option<String>,
}

impl<'a, 'b> HtmlWalker<'a, 'b> {
    fn new(src: &'a str, builder: &'b mut DocumentStructureBuilder) -> Self {
        Self {
            src,
            pos: 0,
            builder,
            text_buf: String::new(),
            inline_stack: Vec::new(),
            annotations: Vec::new(),
            in_pre: false,
            pre_block: None,
            table: None,
            list_stack: Vec::new(),
            in_list_item: false,
            list_item_text: String::new(),
            def_list: None,
            in_dt: false,
            in_dd: false,
            dt_text: String::new(),
            dd_text: String::new(),
            figure: None,
            in_head: false,
            meta_entries: Vec::new(),
            pending_classes: None,
        }
    }

    fn walk(&mut self) {
        while self.pos < self.src.len() {
            if self.src[self.pos..].starts_with("<!--") {
                if let Some(end) = self.src[self.pos..].find("-->") {
                    self.pos += end + 3;
                } else {
                    self.pos = self.src.len();
                }
                continue;
            }

            if self.src.as_bytes()[self.pos] == b'<' {
                self.handle_tag();
            } else {
                self.handle_text();
            }
        }
        self.flush_paragraph();
    }

    fn handle_text(&mut self) {
        let start = self.pos;
        while self.pos < self.src.len() && self.src.as_bytes()[self.pos] != b'<' {
            self.pos += 1;
        }
        let raw = &self.src[start..self.pos];
        let decoded = decode_entities(raw);

        if let Some(ref mut table) = self.table {
            table.push_text(&decoded);
            return;
        }

        if let Some(ref mut pre) = self.pre_block {
            pre.text.push_str(&decoded);
            return;
        }

        if self.in_list_item {
            self.list_item_text.push_str(&decoded);
            return;
        }

        if self.in_dt {
            self.dt_text.push_str(&decoded);
            return;
        }

        if self.in_dd {
            self.dd_text.push_str(&decoded);
            return;
        }

        if let Some(ref mut fig) = self.figure
            && fig.in_caption
        {
            let cap = fig.caption.get_or_insert_with(String::new);
            cap.push_str(&decoded);
            return;
        }

        self.text_buf.push_str(&decoded);
    }

    fn handle_tag(&mut self) {
        let tag_start = self.pos;
        let Some(end) = self.src[self.pos..].find('>') else {
            self.pos = self.src.len();
            return;
        };
        let tag_content = &self.src[self.pos + 1..self.pos + end];
        self.pos += end + 1;

        if tag_content.starts_with('!') || tag_content.starts_with('?') {
            return;
        }

        let is_closing = tag_content.starts_with('/');
        let content = if is_closing { &tag_content[1..] } else { tag_content };

        let content = content.trim_end_matches('/').trim();

        let (tag_name, attrs_str) = split_tag_name(content);
        let tag_lower = tag_name.to_ascii_lowercase();

        if is_closing {
            self.handle_close_tag(&tag_lower, tag_start);
        } else {
            let is_self_closing = tag_content.ends_with('/');
            self.handle_open_tag(&tag_lower, attrs_str, is_self_closing);
        }
    }

    fn handle_open_tag(&mut self, tag: &str, attrs_str: &str, is_self_closing: bool) {
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush_paragraph();
                self.text_buf.clear();
                self.annotations.clear();
                self.pending_classes = extract_attr(attrs_str, "class").map(|s| s.to_string());
            }
            "p" => {
                self.flush_paragraph();
                self.pending_classes = extract_attr(attrs_str, "class").map(|s| s.to_string());
            }
            "br" => {
                if self.in_pre || self.pre_block.is_some() {
                    if let Some(ref mut pre) = self.pre_block {
                        pre.text.push('\n');
                    }
                } else if self.in_list_item {
                    self.list_item_text.push('\n');
                } else {
                    self.text_buf.push('\x01');
                }
            }
            "strong" | "b" => self.push_inline(InlineKind::Bold),
            "em" | "i" => self.push_inline(InlineKind::Italic),
            "code" => {
                if self.in_pre {
                    let lang = extract_attr(attrs_str, "class").and_then(|c| extract_language_from_class(c));
                    self.pre_block = Some(PreBlock {
                        language: lang.map(|s| s.to_string()),
                        text: String::new(),
                    });
                } else {
                    self.push_inline(InlineKind::Code);
                }
            }
            "u" | "ins" => self.push_inline(InlineKind::Underline),
            "s" | "del" | "strike" => self.push_inline(InlineKind::Strikethrough),
            "sub" => self.push_inline(InlineKind::Subscript),
            "sup" => self.push_inline(InlineKind::Superscript),
            "mark" => self.push_inline(InlineKind::Highlight),
            "a" => {
                let href = extract_attr(attrs_str, "href").unwrap_or("").to_string();
                let title = extract_attr(attrs_str, "title").map(|s| s.to_string());
                self.push_inline(InlineKind::Link { href, title });
            }
            "pre" => {
                self.flush_paragraph();
                self.in_pre = true;
                self.pre_block = Some(PreBlock {
                    language: None,
                    text: String::new(),
                });
            }
            "blockquote" => {
                self.flush_paragraph();
                let idx = self.builder.push_quote(None);
                if let Some(cite) = extract_attr(attrs_str, "cite") {
                    let mut attrs = AHashMap::new();
                    attrs.insert("cite".to_string(), cite.to_string());
                    self.builder.set_attributes(idx, attrs);
                }
            }
            "ul" => {
                self.flush_paragraph();
                let idx = self.builder.push_list(false, None);
                self.list_stack.push(ListContext { node_idx: idx });
            }
            "ol" => {
                self.flush_paragraph();
                let idx = self.builder.push_list(true, None);
                if let Some(start_val) = extract_attr(attrs_str, "start") {
                    let mut attrs = AHashMap::new();
                    attrs.insert("start".to_string(), start_val.to_string());
                    self.builder.set_attributes(idx, attrs);
                }
                self.list_stack.push(ListContext { node_idx: idx });
            }
            "li" => {
                self.flush_list_item();
                self.in_list_item = true;
                self.list_item_text.clear();
            }
            "table" => {
                self.flush_paragraph();
                self.table = Some(TableAccumulator::new());
            }
            "tr" | "thead" | "tbody" | "tfoot" => {
                if tag == "tr"
                    && let Some(ref mut table) = self.table
                {
                    table.open_row();
                }
            }
            "th" | "td" => {
                if let Some(ref mut table) = self.table {
                    let col_span = extract_attr(attrs_str, "colspan")
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(1);
                    let row_span = extract_attr(attrs_str, "rowspan")
                        .and_then(|v| v.parse::<u32>().ok())
                        .unwrap_or(1);
                    table.open_cell(col_span, row_span, tag == "th");
                }
            }
            "img" => {
                let alt = extract_attr(attrs_str, "alt");
                let src = extract_attr(attrs_str, "src").map(|s| s.to_string());
                let width = extract_attr(attrs_str, "width").map(|s| s.to_string());
                let height = extract_attr(attrs_str, "height").map(|s| s.to_string());

                if let Some(ref mut fig) = self.figure {
                    fig.img_alt = alt.map(|s| s.to_string());
                    fig.img_src = src;
                    fig.img_width = width;
                    fig.img_height = height;
                } else {
                    self.flush_paragraph();
                    let idx = self.builder.push_image_with_src(alt, src.as_deref(), None, None, None);
                    if width.is_some() || height.is_some() {
                        let mut attrs = AHashMap::new();
                        if let Some(w) = width {
                            attrs.insert("width".to_string(), w);
                        }
                        if let Some(h) = height {
                            attrs.insert("height".to_string(), h);
                        }
                        self.builder.set_attributes(idx, attrs);
                    }
                }
            }
            "figure" => {
                self.flush_paragraph();
                self.figure = Some(FigureContext {
                    img_alt: None,
                    img_src: None,
                    img_width: None,
                    img_height: None,
                    caption: None,
                    in_caption: false,
                });
            }
            "figcaption" => {
                if let Some(ref mut fig) = self.figure {
                    fig.in_caption = true;
                    fig.caption = Some(String::new());
                }
            }
            "dl" => {
                self.flush_paragraph();
                let idx = self.builder.push_definition_list(None);
                self.def_list = Some(DefListContext {
                    list_idx: idx,
                    current_term: None,
                });
            }
            "dt" => {
                self.flush_definition_item();
                self.in_dt = true;
                self.dt_text.clear();
            }
            "dd" => {
                self.in_dt = false;
                if let Some(ref mut dl) = self.def_list {
                    let term = normalize_whitespace(&self.dt_text);
                    if !term.is_empty() {
                        dl.current_term = Some(term);
                    }
                }
                self.dt_text.clear();
                self.in_dd = true;
                self.dd_text.clear();
            }
            "head" => {
                self.in_head = true;
                self.meta_entries.clear();
            }
            "meta" if self.in_head => {
                let name = extract_attr(attrs_str, "name");
                let content_val = extract_attr(attrs_str, "content");
                if let (Some(n), Some(c)) = (name, content_val) {
                    self.meta_entries.push((n.to_string(), c.to_string()));
                }
            }
            "script" | "style" => {
                let close_tag = format!("</{tag}>");
                if let Some(close_pos) = self.src[self.pos..].find(&close_tag) {
                    let block_content = &self.src[self.pos..self.pos + close_pos];
                    self.pos += close_pos + close_tag.len();
                    if !block_content.trim().is_empty() {
                        self.builder.push_raw_block(tag, block_content.trim(), None);
                    }
                }
            }
            "video" | "audio" => {
                let close_tag = format!("</{tag}>");
                if let Some(close_pos) = self.src[self.pos..].find(&close_tag) {
                    self.pos += close_pos + close_tag.len();
                }
            }
            "math" => {
                self.flush_paragraph();
                if !is_self_closing {
                    let close_tag = "</math>";
                    if let Some(close_pos) = self.src[self.pos..].find(close_tag) {
                        let inner = &self.src[self.pos..self.pos + close_pos];
                        let raw_xml = if attrs_str.is_empty() {
                            format!("<math>{inner}</math>")
                        } else {
                            format!("<math {attrs_str}>{inner}</math>")
                        };
                        self.pos += close_pos + close_tag.len();
                        if let Some(latex) = convert_math_subtree_to_latex(&raw_xml) {
                            self.builder.push_formula(&latex, None);
                        }
                    }
                }
            }
            "hr" => {
                self.flush_paragraph();
            }
            "div" | "section" | "article" | "main" | "aside" | "header" | "footer" | "nav" | "details" | "summary" => {
                self.flush_paragraph();
            }
            "span" | "html" | "body" | "title" | "link" => {}
            _ => {}
        }
    }

    fn handle_close_tag(&mut self, tag: &str, _tag_start: usize) {
        match tag {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let level: u8 = tag[1..].parse().unwrap_or(1);
                let text = self.text_buf.trim().to_string();
                if !text.is_empty() {
                    let idx = self.builder.push_heading(level, &text, None, None);
                    if let Some(classes) = self.pending_classes.take() {
                        let mut attrs = AHashMap::new();
                        attrs.insert("class".to_string(), classes);
                        self.builder.set_attributes(idx, attrs);
                    }
                }
                self.text_buf.clear();
                self.annotations.clear();
                self.inline_stack.clear();
            }
            "p" => {
                self.flush_paragraph();
            }
            "strong" | "b" => self.pop_inline(InlineKind::Bold),
            "em" | "i" => self.pop_inline(InlineKind::Italic),
            "code" => {
                if self.in_pre {
                } else {
                    self.pop_inline(InlineKind::Code);
                }
            }
            "u" | "ins" => self.pop_inline(InlineKind::Underline),
            "s" | "del" | "strike" => self.pop_inline(InlineKind::Strikethrough),
            "sub" => self.pop_inline(InlineKind::Subscript),
            "sup" => self.pop_inline(InlineKind::Superscript),
            "mark" => self.pop_inline(InlineKind::Highlight),
            "a" => {
                self.pop_inline_link();
            }
            "pre" => {
                if let Some(pre) = self.pre_block.take() {
                    let text = pre.text.trim_end_matches('\n').to_string();
                    if !text.is_empty() {
                        self.builder.push_code(&text, pre.language.as_deref(), None);
                    }
                }
                self.in_pre = false;
            }
            "blockquote" => {
                self.flush_paragraph();
                self.builder.exit_container();
            }
            "ul" | "ol" => {
                self.flush_list_item();
                self.list_stack.pop();
            }
            "li" => {
                self.flush_list_item();
            }
            "table" => {
                if let Some(mut table) = self.table.take() {
                    table.close_cell();
                    table.close_row();
                    if !table.rows.is_empty() {
                        self.emit_table_with_spans(&table.rows);
                    }
                }
            }
            "tr" => {
                if let Some(ref mut table) = self.table {
                    table.close_cell();
                    table.close_row();
                }
            }
            "th" | "td" => {
                if let Some(ref mut table) = self.table {
                    table.close_cell();
                }
            }
            "dl" => {
                self.flush_definition_item();
                self.def_list = None;
            }
            "dt" => {
                self.in_dt = false;
            }
            "dd" => {
                // `flush_definition_item` gates its `dd` branch on `self.in_dd` still being
                // `true` (it's what tells it there's a pending definition to push) — it must
                // run before that flag is cleared, or the definition item is silently
                // dropped (issue #127: this was the actual reason `<dl>/<dt>/<dd>` content
                // never reached `DefinitionItem` nodes at all).
                self.flush_definition_item();
                self.in_dd = false;
            }
            "figure" => {
                if let Some(fig) = self.figure.take() {
                    let desc = fig
                        .caption
                        .as_deref()
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .or(fig.img_alt.as_deref());
                    let idx = self
                        .builder
                        .push_image_with_src(desc, fig.img_src.as_deref(), None, None, None);
                    let has_dims = fig.img_width.is_some() || fig.img_height.is_some();
                    if has_dims {
                        let mut attrs = AHashMap::new();
                        if let Some(w) = fig.img_width {
                            attrs.insert("width".to_string(), w);
                        }
                        if let Some(h) = fig.img_height {
                            attrs.insert("height".to_string(), h);
                        }
                        self.builder.set_attributes(idx, attrs);
                    }
                }
            }
            "figcaption" => {
                if let Some(ref mut fig) = self.figure {
                    fig.in_caption = false;
                }
            }
            "head" => {
                self.in_head = false;
                if !self.meta_entries.is_empty() {
                    let entries = std::mem::take(&mut self.meta_entries);
                    self.builder.push_metadata_block(entries, None);
                }
            }
            "div" | "section" | "article" | "main" | "aside" | "header" | "footer" | "nav" | "details" | "summary" => {
                self.flush_paragraph();
            }
            _ => {}
        }
    }

    /// Build a `TableGrid` from accumulated rows with colspan/rowspan support.
    fn emit_table_with_spans(&mut self, rows: &[Vec<CellMeta>]) {
        use crate::types::document_structure::{GridCell, TableGrid};

        let num_rows = rows.len() as u32;

        let has_spans = rows.iter().any(|r| r.iter().any(|c| c.col_span > 1 || c.row_span > 1));

        if !has_spans {
            let simple: Vec<Vec<String>> = rows
                .iter()
                .map(|r| r.iter().map(|c| c.text.clone()).collect())
                .collect();
            self.builder.push_table_from_cells(&simple, None);
            return;
        }

        let mut grid_cells = Vec::new();
        let cols = crate::extraction::grid_flatten::resolve_span_grid(
            rows,
            |c| c.col_span,
            |c| c.row_span,
            |row_idx, col, cell| {
                grid_cells.push(GridCell {
                    content: cell.text.clone(),
                    row: row_idx,
                    col,
                    row_span: cell.row_span,
                    col_span: cell.col_span,
                    is_header: cell.is_header,
                    bbox: None,
                });
            },
        );

        let grid = TableGrid {
            rows: num_rows,
            cols,
            cells: grid_cells,
        };
        self.builder.push_table(grid, None, None);
    }

    fn push_inline(&mut self, kind: InlineKind) {
        let offset = if self.in_list_item {
            self.list_item_text.len() as u32
        } else {
            self.text_buf.len() as u32
        };
        self.inline_stack.push(InlineSpan {
            kind,
            text_start: offset,
        });
    }

    fn pop_inline(&mut self, expected: InlineKind) {
        let idx = self
            .inline_stack
            .iter()
            .rposition(|s| std::mem::discriminant(&s.kind) == std::mem::discriminant(&expected));
        if let Some(i) = idx {
            let span = self.inline_stack.remove(i);
            let end = if self.in_list_item {
                self.list_item_text.len() as u32
            } else {
                self.text_buf.len() as u32
            };
            if end > span.text_start {
                let annotation = match span.kind {
                    InlineKind::Bold => builder::bold(span.text_start, end),
                    InlineKind::Italic => builder::italic(span.text_start, end),
                    InlineKind::Code => builder::code(span.text_start, end),
                    InlineKind::Underline => builder::underline(span.text_start, end),
                    InlineKind::Strikethrough => builder::strikethrough(span.text_start, end),
                    InlineKind::Subscript => TextAnnotation {
                        start: span.text_start,
                        end,
                        kind: crate::types::document_structure::AnnotationKind::Subscript,
                    },
                    InlineKind::Superscript => TextAnnotation {
                        start: span.text_start,
                        end,
                        kind: crate::types::document_structure::AnnotationKind::Superscript,
                    },
                    InlineKind::Highlight => TextAnnotation {
                        start: span.text_start,
                        end,
                        kind: crate::types::document_structure::AnnotationKind::Highlight,
                    },
                    InlineKind::Link { .. } => unreachable!("Links handled separately by pop_inline_link"),
                };
                self.annotations.push(annotation);
            }
        }
    }

    fn pop_inline_link(&mut self) {
        let idx = self
            .inline_stack
            .iter()
            .rposition(|s| matches!(s.kind, InlineKind::Link { .. }));
        if let Some(i) = idx {
            let span = self.inline_stack.remove(i);
            let end = if self.in_list_item {
                self.list_item_text.len() as u32
            } else {
                self.text_buf.len() as u32
            };
            if end > span.text_start
                && let InlineKind::Link { href, title } = span.kind
            {
                let annotation = builder::link(span.text_start, end, &href, title.as_deref());
                self.annotations.push(annotation);
            }
        }
    }

    fn flush_paragraph(&mut self) {
        let text = normalize_whitespace(&self.text_buf);
        if !text.is_empty() {
            let annotations = std::mem::take(&mut self.annotations);
            let idx = self.builder.push_paragraph(&text, annotations, None, None);
            if let Some(classes) = self.pending_classes.take() {
                let mut attrs = AHashMap::new();
                attrs.insert("class".to_string(), classes);
                self.builder.set_attributes(idx, attrs);
            }
        }
        self.text_buf.clear();
        self.annotations.clear();
        self.inline_stack.clear();
    }

    fn flush_list_item(&mut self) {
        if !self.in_list_item {
            return;
        }
        self.in_list_item = false;
        let text = normalize_whitespace(&self.list_item_text);
        if !text.is_empty()
            && let Some(ctx) = self.list_stack.last()
        {
            self.builder.push_list_item(ctx.node_idx, &text, None);
        }
        self.list_item_text.clear();
    }

    fn flush_definition_item(&mut self) {
        if self.in_dd {
            self.in_dd = false;
            if let Some(ref mut dl) = self.def_list {
                let definition = normalize_whitespace(&self.dd_text);
                if let Some(term) = dl.current_term.take() {
                    self.builder.push_definition_item(dl.list_idx, &term, &definition, None);
                }
            }
            self.dd_text.clear();
        }
        if self.in_dt {
            self.in_dt = false;
            if let Some(ref mut dl) = self.def_list {
                let term = normalize_whitespace(&self.dt_text);
                if !term.is_empty() {
                    dl.current_term = Some(term);
                }
            }
            self.dt_text.clear();
        }
    }
}

/// Split a tag body into (name, rest-of-attributes).
/// Convert a raw `<math>...</math>` XHTML subtree to LaTeX via the shared
/// MathML converter (`crate::extraction::mathml`, gated by the `office`
/// feature). Returns `None` if the fragment fails to parse, converts to empty
/// output, or the security budget is exhausted on hostile input.
#[cfg(feature = "office")]
fn convert_math_subtree_to_latex(raw_xml: &str) -> Option<String> {
    let mut budget = crate::extractors::security::SecurityBudget::from_limits(
        &crate::extractors::security::SecurityLimits::default(),
    );
    crate::extraction::mathml::convert_mathml_str_to_latex(raw_xml, &mut budget)
        .ok()
        .filter(|latex| !latex.trim().is_empty())
}

/// The `mathml` converter lives behind the `office` feature; without it, `math`
/// elements are dropped rather than mangled into stray inline text.
#[cfg(not(feature = "office"))]
fn convert_math_subtree_to_latex(_raw_xml: &str) -> Option<String> {
    None
}

fn split_tag_name(content: &str) -> (&str, &str) {
    let content = content.trim();
    if let Some(space_pos) = content.find(|c: char| c.is_ascii_whitespace()) {
        (&content[..space_pos], &content[space_pos + 1..])
    } else {
        (content, "")
    }
}

/// Extract an attribute value from a raw attributes string.
///
/// Handles both `attr="value"` and `attr='value'` forms.
fn extract_attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let search = format!("{name}=");
    let mut search_from = 0;
    let idx = loop {
        let candidate = attrs[search_from..].find(&search)?;
        let abs = search_from + candidate;
        if abs == 0 || !attrs.as_bytes()[abs - 1].is_ascii_alphanumeric() {
            break abs;
        }
        search_from = abs + 1;
    };
    let after_eq = &attrs[idx + search.len()..];
    let after_eq = after_eq.trim_start();
    if after_eq.is_empty() {
        return None;
    }
    let quote = after_eq.as_bytes()[0];
    if quote == b'"' || quote == b'\'' {
        let rest = &after_eq[1..];
        let end = rest.find(quote as char)?;
        Some(&rest[..end])
    } else {
        let end = after_eq
            .find(|c: char| c.is_ascii_whitespace() || c == '>')
            .unwrap_or(after_eq.len());
        Some(&after_eq[..end])
    }
}

/// Extract a language identifier from a class attribute like `language-rust` or `lang-python`.
fn extract_language_from_class(class: &str) -> Option<&str> {
    for cls in class.split_ascii_whitespace() {
        if let Some(lang) = cls.strip_prefix("language-") {
            return Some(lang);
        }
        if let Some(lang) = cls.strip_prefix("lang-") {
            return Some(lang);
        }
    }
    None
}

/// Decode basic HTML entities.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            let mut entity = String::new();
            for ec in chars.by_ref() {
                if ec == ';' {
                    break;
                }
                entity.push(ec);
                if entity.len() > 10 {
                    out.push('&');
                    out.push_str(&entity);
                    entity.clear();
                    break;
                }
            }
            if entity.is_empty() {
                continue;
            }
            match entity.as_str() {
                "amp" => out.push('&'),
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                "nbsp" => out.push(' '),
                "copy" => out.push('\u{00A9}'),
                "reg" => out.push('\u{00AE}'),
                "trade" => out.push('\u{2122}'),
                "mdash" => out.push('\u{2014}'),
                "ndash" => out.push('\u{2013}'),
                "laquo" => out.push('\u{00AB}'),
                "raquo" => out.push('\u{00BB}'),
                "hellip" => out.push('\u{2026}'),
                "eacute" => out.push('\u{00E9}'),
                "egrave" => out.push('\u{00E8}'),
                "ecirc" => out.push('\u{00EA}'),
                "euml" => out.push('\u{00EB}'),
                "aacute" => out.push('\u{00E1}'),
                "agrave" => out.push('\u{00E0}'),
                "acirc" => out.push('\u{00E2}'),
                "auml" => out.push('\u{00E4}'),
                "iacute" => out.push('\u{00ED}'),
                "ocirc" => out.push('\u{00F4}'),
                "ouml" => out.push('\u{00F6}'),
                "uuml" => out.push('\u{00FC}'),
                "ntilde" => out.push('\u{00F1}'),
                "ccedil" => out.push('\u{00E7}'),
                "ldquo" => out.push('\u{201C}'),
                "rdquo" => out.push('\u{201D}'),
                "lsquo" => out.push('\u{2018}'),
                "rsquo" => out.push('\u{2019}'),
                "bull" => out.push('\u{2022}'),
                "middot" => out.push('\u{00B7}'),
                "euro" => out.push('\u{20AC}'),
                "pound" => out.push('\u{00A3}'),
                "yen" => out.push('\u{00A5}'),
                "times" => out.push('\u{00D7}'),
                "divide" => out.push('\u{00F7}'),
                "plusmn" => out.push('\u{00B1}'),
                other => {
                    if let Some(num_str) = other.strip_prefix('#') {
                        let code_point = if num_str.starts_with('x') || num_str.starts_with('X') {
                            u32::from_str_radix(&num_str[1..], 16).ok()
                        } else {
                            num_str.parse::<u32>().ok()
                        };
                        if let Some(cp) = code_point
                            && let Some(ch) = char::from_u32(cp)
                        {
                            out.push(ch);
                            continue;
                        }
                    }
                    out.push('&');
                    out.push_str(other);
                    out.push(';');
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Collapse runs of whitespace into single spaces and trim.
///
/// The sentinel character `\x01` marks intentional line breaks inserted by
/// `<br>` tag handling. These are converted to real newlines in the output
/// while all other whitespace (including source HTML newlines) is collapsed.
fn normalize_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = true;

    for c in s.chars() {
        if c == '\x01' {
            while out.ends_with(' ') {
                out.pop();
            }
            out.push('\n');
            last_was_space = true;
        } else if c.is_ascii_whitespace() {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(c);
            last_was_space = false;
        }
    }
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::document_structure::{AnnotationKind, NodeContent};

    #[test]
    fn test_headings() {
        let html = "<h1>Title</h1><h2>Subtitle</h2>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
    }

    #[test]
    fn test_paragraphs() {
        let html = "<p>First paragraph.</p><p>Second paragraph.</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 2);
    }

    #[test]
    fn test_bold_annotation() {
        let html = "<p>Hello <strong>world</strong>!</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());

        let para = &doc.nodes[0];
        if let NodeContent::Paragraph { ref text } = para.content {
            assert_eq!(text, "Hello world!");
        } else {
            panic!("Expected paragraph, got {:?}", para.content);
        }
        assert_eq!(para.annotations.len(), 1);
        assert_eq!(para.annotations[0].kind, AnnotationKind::Bold);
        assert_eq!(para.annotations[0].start, 6);
        assert_eq!(para.annotations[0].end, 11);
    }

    #[test]
    fn test_italic_annotation() {
        let html = "<p><em>italic</em> text</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        assert_eq!(para.annotations.len(), 1);
        assert_eq!(para.annotations[0].kind, AnnotationKind::Italic);
        assert_eq!(para.annotations[0].start, 0);
        assert_eq!(para.annotations[0].end, 6);
    }

    #[test]
    fn test_link_annotation() {
        let html = r#"<p>Click <a href="https://example.com" title="Example">here</a>.</p>"#;
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        assert_eq!(para.annotations.len(), 1);
        match &para.annotations[0].kind {
            AnnotationKind::Link { url, title } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(title.as_deref(), Some("Example"));
            }
            other => panic!("Expected Link annotation, got {:?}", other),
        }
    }

    #[test]
    fn test_code_block() {
        let html = r#"<pre><code class="language-rust">fn main() {}</code></pre>"#;
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let node = &doc.nodes[0];
        match &node.content {
            NodeContent::Code { text, language } => {
                assert_eq!(text, "fn main() {}");
                assert_eq!(language.as_deref(), Some("rust"));
            }
            other => panic!("Expected Code, got {:?}", other),
        }
    }

    #[test]
    #[cfg(feature = "office")]
    fn test_math_converts_to_latex_formula_node() {
        let html = r#"<p>Before</p><math xmlns="http://www.w3.org/1998/Math/MathML"><mfrac><mn>1</mn><mn>2</mn></mfrac></math><p>After</p>"#;
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());

        let formula = doc
            .nodes
            .iter()
            .find_map(|node| match &node.content {
                NodeContent::Formula { text } => Some(text.clone()),
                _ => None,
            })
            .expect("expected a Formula node");
        assert_eq!(formula, "\\frac{1}{2}");

        assert!(
            doc.nodes
                .iter()
                .all(|node| !format!("{:?}", node.content).contains("mfrac")),
            "raw MathML tag names must not leak into any node"
        );
    }

    #[test]
    fn test_unordered_list() {
        let html = "<ul><li>One</li><li>Two</li><li>Three</li></ul>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.len(), 4);
        match &doc.nodes[0].content {
            NodeContent::List { ordered } => assert!(!ordered),
            other => panic!("Expected List, got {:?}", other),
        }
        assert_eq!(doc.nodes[0].children.len(), 3);
    }

    #[test]
    fn test_ordered_list() {
        let html = "<ol><li>First</li><li>Second</li></ol>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        match &doc.nodes[0].content {
            NodeContent::List { ordered } => assert!(ordered),
            other => panic!("Expected List, got {:?}", other),
        }
    }

    #[test]
    fn test_table() {
        let html = "<table><tr><th>Name</th><th>Age</th></tr><tr><td>Alice</td><td>30</td></tr></table>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        match &doc.nodes[0].content {
            NodeContent::Table { grid } => {
                assert_eq!(grid.rows, 2);
                assert_eq!(grid.cols, 2);
            }
            other => panic!("Expected Table, got {:?}", other),
        }
    }

    #[test]
    fn test_blockquote() {
        let html = "<blockquote><p>Quoted text.</p></blockquote>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
        let quote = &doc.nodes[0];
        assert!(matches!(quote.content, NodeContent::Quote));
        assert_eq!(quote.children.len(), 1);
    }

    #[test]
    fn test_blockquote_with_divs() {
        let html = r#"<div>Before</div>
<blockquote><div><div>Line one</div><div>Line two</div></div></blockquote>
<div>After</div>"#;
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok(), "validate: {:?}", doc.validate());

        let roots: Vec<_> = doc.body_roots().collect();
        println!("=== ALL NODES ===");
        for (i, node) in doc.nodes.iter().enumerate() {
            println!(
                "  [{}] {:?} parent={:?} children={:?}",
                i, node.content, node.parent, node.children
            );
        }

        let quote_idx = doc.nodes.iter().position(|n| matches!(n.content, NodeContent::Quote));
        assert!(
            quote_idx.is_some(),
            "Should have a Quote node. Roots: {:?}",
            roots.len()
        );
        let quote = &doc.nodes[quote_idx.unwrap()];
        assert!(
            !quote.children.is_empty(),
            "Quote should have children with div content"
        );

        let child_texts: Vec<_> = quote
            .children
            .iter()
            .filter_map(|ci| match &doc.nodes[ci.0 as usize].content {
                NodeContent::Paragraph { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(child_texts.contains(&"Line one"), "Quote children: {:?}", child_texts);
        assert!(child_texts.contains(&"Line two"), "Quote children: {:?}", child_texts);
    }

    #[test]
    fn test_image() {
        let html = r#"<img src="photo.jpg" alt="A photo">"#;
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        match &doc.nodes[0].content {
            NodeContent::Image { description, .. } => {
                assert_eq!(description.as_deref(), Some("A photo"));
            }
            other => panic!("Expected Image, got {:?}", other),
        }
    }

    #[test]
    fn test_mixed_inline_formatting() {
        let html = "<p><strong>bold</strong> and <em>italic</em> and <code>code</code></p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        assert_eq!(para.annotations.len(), 3);
    }

    #[test]
    fn test_css_class_attribute() {
        let html = r#"<p class="intro highlight">Styled paragraph.</p>"#;
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let node = &doc.nodes[0];
        let attrs = node.attributes.as_ref().expect("attributes should be set");
        assert_eq!(attrs.get("class").unwrap(), "intro highlight");
    }

    #[test]
    fn test_entities_decoded() {
        let html = "<p>Caf&eacute; &amp; Restaurant</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        if let NodeContent::Paragraph { ref text } = para.content {
            assert!(text.contains("Caf\u{00E9}"), "eacute should be decoded");
            assert!(text.contains('&'), "amp should be decoded to &");
            assert!(text.contains("Restaurant"));
        } else {
            panic!("Expected paragraph");
        }
    }

    #[test]
    fn test_nested_headings_structure() {
        let html = "<h1>Top</h1><p>Intro</p><h2>Sub</h2><p>Detail</p><h1>Next</h1><p>More</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 2);
    }

    #[test]
    fn test_source_format_set() {
        let html = "<p>test</p>";
        let doc = build_document_structure(html);
        assert_eq!(doc.source_format.as_deref(), Some("html"));
    }

    #[test]
    fn test_empty_html() {
        let doc = build_document_structure("");
        assert!(doc.validate().is_ok());
        assert!(doc.is_empty());
    }

    #[test]
    fn test_whitespace_only() {
        let doc = build_document_structure("   \n\t  ");
        assert!(doc.validate().is_ok());
        assert!(doc.is_empty());
    }

    #[test]
    fn test_script_becomes_raw_block() {
        let html = "<script>var x = 1;</script><p>Content</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 2);
        match &doc.nodes[0].content {
            NodeContent::RawBlock { format, content } => {
                assert_eq!(format, "script");
                assert!(content.contains("var x"));
            }
            other => panic!("Expected RawBlock, got {:?}", other),
        }
    }

    #[test]
    fn test_strikethrough_annotation() {
        let html = "<p>Some <del>deleted</del> text</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        assert_eq!(para.annotations.len(), 1);
        assert_eq!(para.annotations[0].kind, AnnotationKind::Strikethrough);
    }

    #[test]
    fn test_inline_code_annotation() {
        let html = "<p>Use <code>println!</code> to print</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        assert_eq!(para.annotations.len(), 1);
        assert_eq!(para.annotations[0].kind, AnnotationKind::Code);
    }

    #[test]
    fn test_underline_annotation() {
        let html = "<p><u>underlined</u></p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        assert_eq!(para.annotations.len(), 1);
        assert_eq!(para.annotations[0].kind, AnnotationKind::Underline);
    }

    #[test]
    fn test_unclosed_tags() {
        let html = "<p>Hello <strong>bold text</p><p>Next paragraph</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert!(!doc.is_empty());
    }

    #[test]
    fn test_nested_same_tags() {
        let html = "<p><strong>outer <strong>inner</strong> text</strong></p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        assert!(!para.annotations.is_empty());
    }

    #[test]
    fn test_self_closing_tags() {
        let html = "<p>Before<br/>After</p><hr/><img src='x.png' alt='photo'/>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert!(doc.len() >= 2);
    }

    #[test]
    fn test_nested_blockquotes() {
        let html = "<blockquote><p>Outer</p><blockquote><p>Inner</p></blockquote></blockquote>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
        let outer = &doc.nodes[0];
        assert!(matches!(outer.content, NodeContent::Quote));
        assert!(
            outer.children.len() >= 2,
            "Outer quote should have paragraph + inner quote"
        );
    }

    #[test]
    fn test_numeric_entity_decoding() {
        let html = "<p>&#169; and &#x2014;</p>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        let para = &doc.nodes[0];
        if let NodeContent::Paragraph { ref text } = para.content {
            assert!(
                text.contains('\u{00A9}'),
                "decimal entity should decode to copyright sign"
            );
            assert!(text.contains('\u{2014}'), "hex entity should decode to em dash");
        } else {
            panic!("Expected paragraph");
        }
    }

    #[test]
    fn test_table_missing_cells() {
        let html = "<table><tr><td>A</td><td>B</td><td>C</td></tr><tr><td>X</td></tr></table>";
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        match &doc.nodes[0].content {
            NodeContent::Table { grid } => {
                assert_eq!(grid.rows, 2);
                assert!(grid.cols >= 1);
            }
            other => panic!("Expected Table, got {:?}", other),
        }
    }

    #[test]
    fn test_attr_extraction_no_false_match() {
        assert_eq!(
            extract_attr(r#"subclass="wrong" class="right""#, "class"),
            Some("right")
        );
        assert_eq!(extract_attr(r#"dataclass="wrong""#, "class"), None);
    }

    #[test]
    fn test_complex_document() {
        let html = r#"
        <html>
        <body>
            <h1>Title</h1>
            <p>Introduction with <strong>bold</strong> and <em>italic</em>.</p>
            <h2>Section 1</h2>
            <p>Content of section 1.</p>
            <ul>
                <li>Item A</li>
                <li>Item B</li>
            </ul>
            <h2>Section 2</h2>
            <pre><code class="language-python">print("hello")</code></pre>
            <table>
                <tr><th>Name</th><th>Value</th></tr>
                <tr><td>Key</td><td>123</td></tr>
            </table>
            <blockquote>
                <p>A famous quote.</p>
            </blockquote>
        </body>
        </html>
        "#;
        let doc = build_document_structure(html);
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
        assert!(doc.len() > 10, "Complex doc should have many nodes, got {}", doc.len());
    }

    /// Regression test for issue #127: `flush_definition_item`'s `dd` branch checks
    /// `self.in_dd`, so it must run before the `</dd>` close-tag handler clears that flag —
    /// otherwise the definition item is silently dropped and only an empty `DefinitionList`
    /// marker node is produced.
    #[test]
    fn test_dl_dt_dd_produces_definition_item_node() {
        let html = r#"<html><body><h1>Glossary</h1><dl><dt>DEFTERM</dt><dd>DEFDESCRIPTION explaining the term.</dd></dl></body></html>"#;
        let doc = build_document_structure(html);
        let item = doc
            .nodes
            .iter()
            .find_map(|n| match &n.content {
                NodeContent::DefinitionItem { term, definition } => Some((term.clone(), definition.clone())),
                _ => None,
            })
            .expect("expected a DefinitionItem node");
        assert_eq!(item.0, "DEFTERM");
        assert_eq!(item.1, "DEFDESCRIPTION explaining the term.");
    }
}
