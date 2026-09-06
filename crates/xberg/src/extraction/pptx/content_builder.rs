//! Content builder for accumulating slide output.
//!
//! This module provides utilities for building the final markdown content
//! from slide elements and managing page boundaries.

pub(super) struct ContentBuilder {
    pub(super) content: String,
    pub(super) boundaries: Vec<crate::types::PageBoundary>,
    pub(super) page_contents: Vec<crate::types::PageContent>,
    pub(super) config: Option<crate::core::config::PageConfig>,
    pub(super) plain: bool,
}

impl ContentBuilder {
    pub(super) fn new(plain: bool) -> Self {
        Self {
            content: String::with_capacity(8192),
            boundaries: Vec::new(),
            page_contents: Vec::new(),
            config: None,
            plain,
        }
    }

    pub(super) fn with_page_config(
        capacity: usize,
        config: Option<crate::core::config::PageConfig>,
        plain: bool,
    ) -> Self {
        Self {
            content: String::with_capacity(capacity),
            boundaries: if config.is_some() {
                Vec::new()
            } else {
                Vec::with_capacity(0)
            },
            page_contents: if config.is_some() {
                Vec::new()
            } else {
                Vec::with_capacity(0)
            },
            config,
            plain,
        }
    }

    pub(super) fn start_slide(&mut self, slide_number: u32) -> usize {
        let byte_start = self.content.len();

        if let Some(ref cfg) = self.config
            && cfg.insert_page_markers
        {
            let marker = cfg.marker_format.replace("{page_num}", &slide_number.to_string());
            self.content.push_str(&marker);
        }

        byte_start
    }

    pub(super) fn end_slide(
        &mut self,
        slide_number: u32,
        byte_start: usize,
        slide_content: String,
        speaker_notes: Option<String>,
        section_name: Option<String>,
    ) {
        let byte_end = self.content.len();

        if self.config.is_some() {
            self.boundaries.push(crate::types::PageBoundary {
                byte_start,
                byte_end,
                page_number: slide_number,
            });

            let is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(&slide_content));
            self.page_contents.push(crate::types::PageContent {
                page_number: slide_number,
                content: slide_content,
                tables: Vec::new(),
                image_indices: vec![],
                image_preprocessing: None,
                hierarchy: None,
                is_blank,
                layout_regions: None,
                speaker_notes,
                section_name,
                sheet_name: None,
                ocr_confidence: None,
            });
        }
    }

    pub(super) fn add_slide_header(&mut self, slide_number: u32) {
        self.content.reserve(50);
        self.content.push_str("\n\n<!-- Slide number: ");
        self.content.push_str(&slide_number.to_string());
        self.content.push_str(" -->\n");
    }

    pub(super) fn add_text(&mut self, text: &str) {
        if !text.trim().is_empty() {
            if !self.content.is_empty() && !self.content.ends_with("\n\n") {
                if !self.content.ends_with('\n') {
                    self.content.push('\n');
                }
                self.content.push('\n');
            }
            self.content.push_str(text);
            if !self.content.ends_with('\n') {
                self.content.push('\n');
            }
        }
    }

    pub(super) fn add_title(&mut self, title: &str) {
        if !title.trim().is_empty() {
            if !self.plain {
                self.content.push_str("# ");
            }
            self.content.push_str(title.trim());
            self.content.push_str("\n\n");
        }
    }

    pub(super) fn add_table(&mut self, rows: &[Vec<String>]) {
        if rows.is_empty() {
            return;
        }

        let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if num_cols == 0 {
            return;
        }

        self.content.push('\n');

        if self.plain {
            let owned: Vec<Vec<String>> = rows.to_vec();
            self.content.push_str(&crate::extraction::cells_to_text(&owned));
        } else {
            let mut col_widths = vec![3usize; num_cols];
            for row in rows {
                for (i, cell) in row.iter().enumerate() {
                    col_widths[i] = col_widths[i].max(cell.len());
                }
            }

            for (row_idx, row) in rows.iter().enumerate() {
                self.content.push('|');
                for (i, cell) in row.iter().enumerate() {
                    let width = col_widths.get(i).copied().unwrap_or(3);
                    self.content.push_str(&format!(" {:width$} |", cell, width = width));
                }
                for i in row.len()..num_cols {
                    let width = col_widths.get(i).copied().unwrap_or(3);
                    self.content.push_str(&format!(" {:width$} |", "", width = width));
                }
                self.content.push('\n');

                if row_idx == 0 {
                    self.content.push('|');
                    for i in 0..num_cols {
                        let width = col_widths.get(i).copied().unwrap_or(3);
                        self.content.push_str(&format!(" {} |", "-".repeat(width)));
                    }
                    self.content.push('\n');
                }
            }
        }
    }

    pub(super) fn add_list_item(&mut self, level: u32, is_ordered: bool, text: &str) {
        if !self.plain {
            let indent_count = level.saturating_sub(1) as usize;
            for _ in 0..indent_count {
                self.content.push_str("  ");
            }

            let marker = if is_ordered { "1." } else { "-" };
            self.content.push_str(marker);
            self.content.push(' ');
        }
        self.content.push_str(text.trim());
        self.content.push('\n');
    }

    pub(super) fn add_image_with_desc(&mut self, _image_id: &str, description: Option<&str>, target: &str) {
        if !self.plain {
            let alt = description
                .map(|d| d.replace('\n', " ").replace('\r', ""))
                .unwrap_or_default();
            let src = if target.is_empty() {
                String::new()
            } else {
                target.to_string()
            };
            if !self.content.is_empty() && !self.content.ends_with('\n') {
                self.content.push('\n');
            }
            self.content.push_str(&format!("![{}]({})\n", alt.trim(), src));
        }
    }

    pub(super) fn add_notes(&mut self, notes: &str) {
        if !notes.trim().is_empty() {
            if self.plain {
                self.content.push_str("\n\nNotes:\n");
            } else {
                self.content.push_str("\n\n### Notes:\n");
            }
            self.content.push_str(notes);
            self.content.push('\n');
        }
    }

    pub(super) fn build(
        self,
    ) -> (
        String,
        Option<Vec<crate::types::PageBoundary>>,
        Option<Vec<crate::types::PageContent>>,
    ) {
        let content = self.content.trim().to_string();
        let boundaries = if self.config.is_some() && !self.boundaries.is_empty() {
            Some(self.boundaries)
        } else {
            None
        };
        let pages = if self.config.is_some() && !self.page_contents.is_empty() {
            Some(self.page_contents)
        } else {
            None
        };
        (content, boundaries, pages)
    }
}
