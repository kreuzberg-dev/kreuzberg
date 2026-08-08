//! Ergonomic builder for constructing `DocumentStructure` trees.
//!
//! Encapsulates the heading-driven section-stack nesting pattern used by
//! extractors to build hierarchical document trees. Every extractor should
//! use this builder instead of hand-rolling tree construction.
//!
//! # Example
//!
//! Not run as a doctest: the builder's constructor and `push_*` / `build` methods are
//! `pub(crate)` — it is the in-crate API extractors build trees with. Downstream crates
//! consume the finished [`super::document_structure::DocumentStructure`] instead.
//!
//! ```ignore
//! use xberg::types::builder::DocumentStructureBuilder;
//!
//! let mut b = DocumentStructureBuilder::new();
//! b.push_heading(1, "Introduction", None, None);
//! b.push_paragraph("First paragraph.", vec![], None, None);
//! b.push_heading(2, "Details", None, None);
//! b.push_paragraph("Sub-section content.", vec![], None, None);
//! let doc = b.build();
//! assert!(doc.validate().is_ok());
//! ```

use super::document_structure::{AnnotationKind, TextAnnotation};

#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
use ahash::AHashMap;

#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
use super::document_structure::{
    ContentLayer, DocumentNode, DocumentStructure, GridCell, NodeContent, NodeId, NodeIndex, TableGrid,
};
#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
use super::extraction::BoundingBox;
#[cfg_attr(alef, alef(skip))]
/// Builder for constructing `DocumentStructure` trees with automatic
/// heading-driven section nesting.
///
/// The builder maintains an internal section stack: when you push a heading,
/// it automatically creates a `Group` container and nests subsequent content
/// under it. Higher-level headings pop deeper sections off the stack.
#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
pub struct DocumentStructureBuilder {
    doc: DocumentStructure,
    section_stack: Vec<(u8, NodeIndex)>,
    /// Stack of active container nodes (Quote, Admonition, Slide, etc.).
    /// When non-empty, body nodes are parented under the top container
    /// instead of the section stack.
    container_stack: Vec<NodeIndex>,
    node_count: u32,
}

#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
impl DocumentStructureBuilder {
    /// Create a new empty builder.
    pub(crate) fn new() -> Self {
        Self {
            doc: DocumentStructure::new(),
            section_stack: Vec::new(),
            container_stack: Vec::new(),
            node_count: 0,
        }
    }

    /// Set the source format identifier (e.g. "docx", "html", "pptx").
    pub(crate) fn source_format(mut self, format: impl Into<String>) -> Self {
        self.doc.source_format = Some(format.into());
        self
    }

    /// Consume the builder and return the constructed `DocumentStructure`.
    pub(crate) fn build(self) -> DocumentStructure {
        debug_assert!(
            self.doc.validate().is_ok(),
            "DocumentStructure validation failed: {:?}",
            self.doc.validate()
        );
        let mut doc = self.doc;
        doc.finalize_node_types();
        doc
    }

    /// Push a heading, creating a `Group` container with automatic section nesting.
    ///
    /// Headings at the same or deeper level pop existing sections. Content
    /// pushed after this heading will be nested under its `Group` node.
    ///
    /// Returns the `NodeIndex` of the `Group` node (not the heading child).
    pub(crate) fn push_heading(
        &mut self,
        level: u8,
        text: &str,
        page: Option<u32>,
        bbox: Option<BoundingBox>,
    ) -> NodeIndex {
        while self.section_stack.last().is_some_and(|(l, _)| *l >= level) {
            self.section_stack.pop();
        }

        let group_content = NodeContent::Group {
            label: None,
            heading_level: Some(level),
            heading_text: Some(text.to_string()),
        };

        let group_idx = self.push_node_raw(group_content, page, bbox, ContentLayer::Body, vec![]);

        if let Some((_, parent_idx)) = self.section_stack.last() {
            self.doc.add_child(*parent_idx, group_idx);
        } else if let Some(container_idx) = self.container_stack.last() {
            self.doc.add_child(*container_idx, group_idx);
        }

        let heading_content = NodeContent::Heading {
            level,
            text: text.to_string(),
        };
        let heading_idx = self.push_node_raw(heading_content, page, bbox, ContentLayer::Body, vec![]);
        self.doc.add_child(group_idx, heading_idx);

        self.section_stack.push((level, group_idx));
        group_idx
    }

    /// Push a paragraph node. Nested under current section if one exists.
    pub(crate) fn push_paragraph(
        &mut self,
        text: &str,
        annotations: Vec<TextAnnotation>,
        page: Option<u32>,
        bbox: Option<BoundingBox>,
    ) -> NodeIndex {
        let content = NodeContent::Paragraph { text: text.to_string() };
        self.push_body_node(content, page, bbox, annotations)
    }

    /// Push a list container. Returns the `NodeIndex` to use with `push_list_item`.
    pub(crate) fn push_list(&mut self, ordered: bool, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::List { ordered };
        self.push_body_node(content, page, None, vec![])
    }

    /// Push a list item as a child of the given list node.
    pub(crate) fn push_list_item(&mut self, list: NodeIndex, text: &str, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::ListItem { text: text.to_string() };
        let idx = self.push_node_raw(content, page, None, ContentLayer::Body, vec![]);
        self.doc.add_child(list, idx);
        idx
    }

    /// Push a table node with a structured grid.
    pub(crate) fn push_table(&mut self, grid: TableGrid, page: Option<u32>, bbox: Option<BoundingBox>) -> NodeIndex {
        let content = NodeContent::Table { grid };
        self.push_body_node(content, page, bbox, vec![])
    }

    /// Push a table from a simple cell grid (`Vec<Vec<String>>`).
    ///
    /// Assumes the first row is the header row.
    pub(crate) fn push_table_from_cells(&mut self, cells: &[Vec<String>], page: Option<u32>) -> NodeIndex {
        let grid = cells_to_grid(cells);
        self.push_table(grid, page, None)
    }

    /// Push a code block.
    pub(crate) fn push_code(&mut self, text: &str, language: Option<&str>, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::Code {
            text: text.to_string(),
            language: language.map(|s| s.to_string()),
        };
        self.push_body_node(content, page, None, vec![])
    }

    /// Push a math formula node (LaTeX text).
    pub(crate) fn push_formula(&mut self, text: &str, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::Formula { text: text.to_string() };
        self.push_body_node(content, page, None, vec![])
    }

    /// Push an image reference node.
    pub(crate) fn push_image(
        &mut self,
        description: Option<&str>,
        image_index: Option<u32>,
        page: Option<u32>,
        bbox: Option<BoundingBox>,
    ) -> NodeIndex {
        let content = NodeContent::Image {
            description: description.map(|s| s.to_string()),
            image_index,
            src: None,
        };
        self.push_body_node(content, page, bbox, vec![])
    }

    /// Push an image node with source URL.
    pub(crate) fn push_image_with_src(
        &mut self,
        description: Option<&str>,
        src: Option<&str>,
        image_index: Option<u32>,
        page: Option<u32>,
        bbox: Option<BoundingBox>,
    ) -> NodeIndex {
        let content = NodeContent::Image {
            description: description.map(|s| s.to_string()),
            image_index,
            src: src.map(|s| s.to_string()),
        };
        self.push_body_node(content, page, bbox, vec![])
    }

    /// Push a block quote container and enter it.
    ///
    /// Subsequent body nodes will be parented under this quote until
    /// [`exit_container`](Self::exit_container) is called.
    pub(crate) fn push_quote(&mut self, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::Quote;
        let idx = self.push_body_node(content, page, None, vec![]);
        self.container_stack.push(idx);
        idx
    }

    /// Push a page break marker (always root-level, never nested under sections).
    #[cfg(test)]
    pub(crate) fn push_page_break(&mut self, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::PageBreak;
        self.push_node_raw(content, page, None, ContentLayer::Body, vec![])
    }

    /// Push a slide container (PPTX) and enter it.
    ///
    /// Clears the section stack and container stack so the slide starts
    /// fresh. Subsequent body nodes will be parented under this slide
    /// until [`exit_container`](Self::exit_container) is called or a new
    /// slide is pushed.
    pub(crate) fn push_slide(&mut self, number: u32, title: Option<&str>) -> NodeIndex {
        self.section_stack.clear();
        self.container_stack.clear();

        let content = NodeContent::Slide {
            number,
            title: title.map(|s| s.to_string()),
        };
        let idx = self.push_node_raw(content, None, None, ContentLayer::Body, vec![]);
        self.container_stack.push(idx);
        idx
    }

    /// Push a definition list container. Use `push_definition_item` for entries.
    pub(crate) fn push_definition_list(&mut self, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::DefinitionList;
        self.push_body_node(content, page, None, vec![])
    }

    /// Push a definition item as a child of the given definition list.
    pub(crate) fn push_definition_item(
        &mut self,
        list: NodeIndex,
        term: &str,
        definition: &str,
        page: Option<u32>,
    ) -> NodeIndex {
        let content = NodeContent::DefinitionItem {
            term: term.to_string(),
            definition: definition.to_string(),
        };
        let idx = self.push_node_raw(content, page, None, ContentLayer::Body, vec![]);
        self.doc.add_child(list, idx);
        idx
    }

    /// Push a citation / bibliographic reference.
    #[expect(
        dead_code,
        reason = "called from citation/bibtex/rst extractors behind the office feature gate"
    )]
    pub(crate) fn push_citation(&mut self, key: &str, text: &str, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::Citation {
            key: key.to_string(),
            text: text.to_string(),
        };
        self.push_body_node(content, page, None, vec![])
    }

    /// Push an admonition container (note, warning, tip, etc.) and enter it.
    ///
    /// Subsequent body nodes will be parented under this admonition until
    /// [`exit_container`](Self::exit_container) is called.
    #[allow(
        dead_code,
        reason = "kept for API parity with InternalDocumentBuilder; reserved for future structured extractors"
    )]
    pub(crate) fn push_admonition(&mut self, kind: &str, title: Option<&str>, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::Admonition {
            kind: kind.to_string(),
            title: title.map(|s| s.to_string()),
        };
        let idx = self.push_body_node(content, page, None, vec![]);
        self.container_stack.push(idx);
        idx
    }

    /// Push a raw block preserved verbatim from the source format.
    pub(crate) fn push_raw_block(&mut self, format: &str, content: &str, page: Option<u32>) -> NodeIndex {
        let node_content = NodeContent::RawBlock {
            format: format.to_string(),
            content: content.to_string(),
        };
        self.push_body_node(node_content, page, None, vec![])
    }

    /// Push a metadata block (email headers, frontmatter key-value pairs).
    pub(crate) fn push_metadata_block(&mut self, entries: Vec<(String, String)>, page: Option<u32>) -> NodeIndex {
        let content = NodeContent::MetadataBlock { entries };
        self.push_body_node(content, page, None, vec![])
    }

    /// Set format-specific attributes on an existing node.
    pub(crate) fn set_attributes(&mut self, index: NodeIndex, attrs: AHashMap<String, String>) {
        if let Some(node) = self.doc.nodes.get_mut(index.0 as usize) {
            node.attributes = Some(attrs.into_iter().collect());
        }
    }

    /// Push a raw `NodeContent` with full control over content layer and annotations.
    /// Nests under current section unless the content type is a root-level type.
    pub(crate) fn push_raw(
        &mut self,
        content: NodeContent,
        page: Option<u32>,
        bbox: Option<BoundingBox>,
        layer: ContentLayer,
        annotations: Vec<TextAnnotation>,
    ) -> NodeIndex {
        match layer {
            ContentLayer::Body => {
                if is_always_root(&content) {
                    self.push_node_raw(content, page, bbox, layer, annotations)
                } else {
                    let idx = self.push_node_raw(content, page, bbox, layer, annotations);
                    if let Some((_, parent_idx)) = self.section_stack.last() {
                        self.doc.add_child(*parent_idx, idx);
                    } else if let Some(container_idx) = self.container_stack.last() {
                        self.doc.add_child(*container_idx, idx);
                    }
                    idx
                }
            }
            _ => self.push_node_raw(content, page, bbox, layer, annotations),
        }
    }

    /// Manually push a node onto the container stack.
    ///
    /// Subsequent body nodes will be parented under this container
    /// until [`exit_container`](Self::exit_container) is called.
    #[cfg(test)]
    pub(crate) fn enter_container(&mut self, container: NodeIndex) {
        self.container_stack.push(container);
    }

    /// Pop the most recent container from the container stack.
    ///
    /// Body nodes will resume parenting under the next container on the
    /// stack, or under the section stack if the container stack is empty.
    pub(crate) fn exit_container(&mut self) {
        self.container_stack.pop();
    }

    /// Push a body-layer node, nesting under the current section first
    /// (for heading-driven nesting), then falling back to the active
    /// container, then root-level.
    fn push_body_node(
        &mut self,
        content: NodeContent,
        page: Option<u32>,
        bbox: Option<BoundingBox>,
        annotations: Vec<TextAnnotation>,
    ) -> NodeIndex {
        let idx = self.push_node_raw(content, page, bbox, ContentLayer::Body, annotations);
        if let Some((_, parent_idx)) = self.section_stack.last() {
            self.doc.add_child(*parent_idx, idx);
        } else if let Some(container_idx) = self.container_stack.last() {
            self.doc.add_child(*container_idx, idx);
        }
        idx
    }

    /// Low-level node push — no automatic parenting.
    fn push_node_raw(
        &mut self,
        content: NodeContent,
        page: Option<u32>,
        bbox: Option<BoundingBox>,
        content_layer: ContentLayer,
        annotations: Vec<TextAnnotation>,
    ) -> NodeIndex {
        let node_type = content.node_type_str();
        let text = content.text().unwrap_or("");
        let index = self.node_count;
        self.node_count += 1;

        let node = DocumentNode {
            id: NodeId::generate(node_type, text, page, index).to_string(),
            content,
            parent: None,
            children: vec![],
            content_layer,
            page,
            page_end: None,
            bbox,
            annotations,
            attributes: None,
        };

        self.doc.push_node(node)
    }
}

#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
impl Default for DocumentStructureBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a simple cell grid to a `TableGrid`.
#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
fn cells_to_grid(cells: &[Vec<String>]) -> TableGrid {
    let rows = cells.len() as u32;
    let cols = cells.iter().map(|r| r.len()).max().unwrap_or(0) as u32;

    let mut grid_cells = Vec::new();
    for (row_idx, row) in cells.iter().enumerate() {
        for (col_idx, content) in row.iter().enumerate() {
            grid_cells.push(GridCell {
                content: content.clone(),
                row: row_idx as u32,
                col: col_idx as u32,
                row_span: 1,
                col_span: 1,
                is_header: row_idx == 0,
                bbox: None,
            });
        }
    }

    TableGrid {
        rows,
        cols,
        cells: grid_cells,
    }
}

/// Check if a content type should always be root-level (not nested under sections).
#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
fn is_always_root(content: &NodeContent) -> bool {
    matches!(content, NodeContent::PageBreak)
}

/// Create a bold annotation for the given byte range.
pub(crate) fn bold(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Bold,
    }
}

/// Create an italic annotation for the given byte range.
pub(crate) fn italic(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Italic,
    }
}

/// Create an underline annotation for the given byte range.
#[cfg(any(feature = "office", feature = "email", feature = "xml"))]
pub(crate) fn underline(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Underline,
    }
}

/// Create a link annotation for the given byte range.
pub(crate) fn link(start: u32, end: u32, url: &str, title: Option<&str>) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Link {
            url: url.to_string(),
            title: title.map(|s| s.to_string()),
        },
    }
}

/// Create a code (inline) annotation for the given byte range.
pub(crate) fn code(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Code,
    }
}

/// Create a strikethrough annotation for the given byte range.
pub(crate) fn strikethrough(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Strikethrough,
    }
}

/// Create a subscript annotation for the given byte range.
pub(crate) fn subscript(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Subscript,
    }
}

/// Create a superscript annotation for the given byte range.
pub(crate) fn superscript(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Superscript,
    }
}

/// Create a font size annotation for the given byte range.
#[cfg(feature = "office")]
pub(crate) fn font_size(start: u32, end: u32, value: &str) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::FontSize {
            value: value.to_string(),
        },
    }
}

/// Create a color annotation for the given byte range.
#[cfg(feature = "office")]
pub(crate) fn color(start: u32, end: u32, value: &str) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Color {
            value: value.to_string(),
        },
    }
}

/// Create a highlight annotation for the given byte range.
#[cfg(feature = "office")]
pub(crate) fn highlight(start: u32, end: u32) -> TextAnnotation {
    TextAnnotation {
        start,
        end,
        kind: AnnotationKind::Highlight,
    }
}

#[cfg(all(test, any(feature = "office", feature = "email", feature = "xml")))]
mod tests {
    use super::*;

    #[test]
    fn test_empty_builder() {
        let doc = DocumentStructureBuilder::new().build();
        assert!(doc.validate().is_ok());
        assert!(doc.is_empty());
    }

    #[test]
    fn test_flat_paragraphs() {
        let mut b = DocumentStructureBuilder::new();
        b.push_paragraph("First", vec![], Some(1), None);
        b.push_paragraph("Second", vec![], Some(1), None);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.len(), 2);
        assert_eq!(doc.body_roots().count(), 2);
    }

    #[test]
    fn test_heading_nesting() {
        let mut b = DocumentStructureBuilder::new();
        b.push_heading(1, "Title", Some(1), None);
        b.push_paragraph("Under h1", vec![], Some(1), None);
        b.push_heading(2, "Subtitle", Some(1), None);
        b.push_paragraph("Under h2", vec![], Some(1), None);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
        assert_eq!(doc.nodes[0].children.len(), 3);
    }

    #[test]
    fn test_heading_same_level_pops() {
        let mut b = DocumentStructureBuilder::new();
        b.push_heading(1, "Section A", None, None);
        b.push_paragraph("A content", vec![], None, None);
        b.push_heading(1, "Section B", None, None);
        b.push_paragraph("B content", vec![], None, None);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 2);
    }

    #[test]
    fn test_list_construction() {
        let mut b = DocumentStructureBuilder::new();
        let list = b.push_list(false, Some(1));
        b.push_list_item(list, "Item 1", Some(1));
        b.push_list_item(list, "Item 2", Some(1));
        b.push_list_item(list, "Item 3", Some(1));
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.len(), 4);
        assert_eq!(doc.nodes[0].children.len(), 3);
    }

    #[test]
    fn test_table_simple() {
        let mut b = DocumentStructureBuilder::new();
        b.push_table_from_cells(
            &[
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            Some(1),
        );
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.len(), 1);
        if let NodeContent::Table { ref grid } = doc.nodes[0].content {
            assert_eq!(grid.rows, 2);
            assert_eq!(grid.cols, 2);
            assert!(grid.cells[0].is_header);
        } else {
            panic!("Expected Table node");
        }
    }

    #[test]
    fn test_slide_captures_children() {
        let mut b = DocumentStructureBuilder::new();
        b.push_slide(1, Some("Slide 1"));
        b.push_heading(1, "Title", None, None);
        b.push_paragraph("Content", vec![], None, None);
        b.exit_container();
        b.push_slide(2, Some("Slide 2"));
        b.push_paragraph("Slide 2 content", vec![], None, None);
        b.exit_container();
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 2);

        let slide1 = &doc.nodes[0];
        assert_eq!(slide1.children.len(), 1);
        let group = &doc.nodes[slide1.children[0].0 as usize];
        assert_eq!(group.children.len(), 2);

        let (_, slide2) = doc.body_roots().nth(1).unwrap();
        assert_eq!(slide2.children.len(), 1);
    }

    #[test]
    fn test_annotations_preserved() {
        let mut b = DocumentStructureBuilder::new();
        let annotations = vec![bold(0, 5), italic(6, 11)];
        b.push_paragraph("Hello World", annotations, Some(1), None);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.nodes[0].annotations.len(), 2);
    }

    #[test]
    fn test_source_format() {
        let doc = DocumentStructureBuilder::new().source_format("docx").build();
        assert_eq!(doc.source_format.as_deref(), Some("docx"));
    }

    #[test]
    fn test_attributes() {
        let mut b = DocumentStructureBuilder::new();
        let idx = b.push_paragraph("styled", vec![], None, None);
        let mut attrs = AHashMap::new();
        attrs.insert("class".to_string(), "highlight".to_string());
        b.set_attributes(idx, attrs);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        let node_attrs = doc.nodes[0].attributes.as_ref().unwrap();
        assert_eq!(node_attrs.get("class").unwrap(), "highlight");
    }

    #[test]
    fn test_definition_list() {
        let mut b = DocumentStructureBuilder::new();
        let dl = b.push_definition_list(Some(1));
        b.push_definition_item(dl, "Term 1", "Definition 1", Some(1));
        b.push_definition_item(dl, "Term 2", "Definition 2", Some(1));
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.len(), 3);
        assert_eq!(doc.nodes[0].children.len(), 2);
    }

    #[test]
    fn test_admonition_captures_children() {
        let mut b = DocumentStructureBuilder::new();
        let adm = b.push_admonition("warning", Some("Be careful"), Some(1));
        b.push_paragraph("Warning content", vec![], Some(1), None);
        b.exit_container();
        let doc = b.build();

        assert!(doc.validate().is_ok());
        match &doc.nodes[adm.0 as usize].content {
            NodeContent::Admonition { kind, title } => {
                assert_eq!(kind, "warning");
                assert_eq!(title.as_deref(), Some("Be careful"));
            }
            _ => panic!("Expected Admonition"),
        }
        assert_eq!(doc.body_roots().count(), 1);
        assert_eq!(doc.nodes[adm.0 as usize].children.len(), 1);
    }

    #[test]
    fn test_serde_roundtrip() {
        let mut b = DocumentStructureBuilder::new().source_format("test");
        b.push_heading(1, "Title", Some(1), None);
        b.push_paragraph("Content", vec![bold(0, 7)], Some(1), None);
        let list = b.push_list(true, Some(1));
        b.push_list_item(list, "One", Some(1));
        b.push_list_item(list, "Two", Some(1));
        b.push_code("fn main() {}", Some("rust"), Some(1));
        let doc = b.build();

        let json = serde_json::to_string(&doc).expect("serialize");
        let deserialized: DocumentStructure = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.len(), doc.len());
        assert!(deserialized.validate().is_ok());
        assert_eq!(deserialized.source_format.as_deref(), Some("test"));
    }

    #[test]
    fn test_page_break_always_root() {
        let mut b = DocumentStructureBuilder::new();
        b.push_heading(1, "Section", Some(1), None);
        b.push_paragraph("Content", vec![], Some(1), None);
        b.push_page_break(Some(1));
        b.push_paragraph("After break", vec![], Some(2), None);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        let page_break = doc.nodes.iter().find(|n| matches!(n.content, NodeContent::PageBreak));
        assert!(page_break.is_some());
        assert!(page_break.unwrap().parent.is_none());
    }

    #[test]
    fn test_annotation_helpers() {
        let b = bold(0, 5);
        assert_eq!(b.start, 0);
        assert_eq!(b.end, 5);
        assert_eq!(b.kind, AnnotationKind::Bold);

        let i = italic(5, 10);
        assert_eq!(i.kind, AnnotationKind::Italic);

        let u = underline(0, 3);
        assert_eq!(u.kind, AnnotationKind::Underline);

        let l = link(0, 5, "https://example.com", Some("Example"));
        match l.kind {
            AnnotationKind::Link { ref url, ref title } => {
                assert_eq!(url, "https://example.com");
                assert_eq!(title.as_deref(), Some("Example"));
            }
            _ => panic!("Expected Link"),
        }

        let c = code(0, 5);
        assert_eq!(c.kind, AnnotationKind::Code);

        let s = strikethrough(0, 5);
        assert_eq!(s.kind, AnnotationKind::Strikethrough);
    }

    #[test]
    fn test_quote_with_nested_paragraphs() {
        let mut b = DocumentStructureBuilder::new();
        let q = b.push_quote(Some(1));
        b.push_paragraph("First quoted line.", vec![], Some(1), None);
        b.push_paragraph("Second quoted line.", vec![], Some(1), None);
        b.exit_container();
        b.push_paragraph("Not in quote.", vec![], Some(1), None);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 2);
        assert_eq!(doc.nodes[q.0 as usize].children.len(), 2);
    }

    #[test]
    fn test_admonition_with_nested_content() {
        let mut b = DocumentStructureBuilder::new();
        let adm = b.push_admonition("note", Some("Important"), Some(1));
        b.push_paragraph("Note body.", vec![], Some(1), None);
        b.push_code("let x = 1;", Some("rust"), Some(1));
        b.exit_container();
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
        assert_eq!(doc.nodes[adm.0 as usize].children.len(), 2);
    }

    #[test]
    fn test_slide_with_headings_and_paragraphs() {
        let mut b = DocumentStructureBuilder::new();
        b.push_slide(1, Some("Intro"));
        b.push_heading(1, "Welcome", None, None);
        b.push_paragraph("Hello.", vec![], None, None);
        b.push_heading(2, "Details", None, None);
        b.push_paragraph("More info.", vec![], None, None);
        b.exit_container();
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
        let slide = &doc.nodes[0];
        assert_eq!(slide.children.len(), 1);
        let h1_group = &doc.nodes[slide.children[0].0 as usize];
        assert_eq!(h1_group.children.len(), 3);
    }

    #[test]
    fn test_nested_containers_slide_containing_quote() {
        let mut b = DocumentStructureBuilder::new();
        b.push_slide(1, Some("Slide 1"));
        b.push_paragraph("Before quote.", vec![], None, None);
        let q = b.push_quote(None);
        b.push_paragraph("Quoted text.", vec![], None, None);
        b.exit_container();
        b.push_paragraph("After quote.", vec![], None, None);
        b.exit_container();
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
        let slide = &doc.nodes[0];
        assert_eq!(slide.children.len(), 3);
        assert_eq!(doc.nodes[q.0 as usize].children.len(), 1);
    }

    #[test]
    fn test_enter_exit_container_manual() {
        let mut b = DocumentStructureBuilder::new();
        let content = NodeContent::Quote;
        let q = b.push_raw(content, Some(1), None, ContentLayer::Body, vec![]);
        b.enter_container(q);
        b.push_paragraph("Inside.", vec![], Some(1), None);
        b.exit_container();
        b.push_paragraph("Outside.", vec![], Some(1), None);
        let doc = b.build();

        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 2);
        assert_eq!(doc.nodes[q.0 as usize].children.len(), 1);
    }

    #[test]
    fn test_exit_container_on_empty_stack_is_noop() {
        let mut b = DocumentStructureBuilder::new();
        b.exit_container();
        b.push_paragraph("Still works.", vec![], None, None);
        let doc = b.build();
        assert!(doc.validate().is_ok());
        assert_eq!(doc.body_roots().count(), 1);
    }
}
