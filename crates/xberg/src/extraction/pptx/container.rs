//! PPTX container and ZIP archive management.
//!
//! This module handles opening PPTX files, reading files from the ZIP archive,
//! finding slide paths, and iterating through slides.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use zip::ZipArchive;

use super::elements::{Slide, SlideElement};
use super::image_handling::get_full_image_path;
use crate::core::diagnostics::push_warning;
use crate::error::{Result, XbergError};
use crate::types::ProcessingWarning;

pub(super) struct PptxContainer<R: Read + Seek> {
    pub(super) archive: ZipArchive<R>,
    slide_paths: Vec<String>,
}

impl PptxContainer<std::fs::File> {
    pub(super) fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        // IO errors must bubble up unchanged - file access issues need user reports ~keep
        let file = std::fs::File::open(path)?;

        let mut archive = match ZipArchive::new(file) {
            Ok(arc) => arc,
            Err(zip::result::ZipError::Io(io_err)) => return Err(io_err.into()), // Bubble up IO errors ~keep
            Err(e) => {
                return Err(XbergError::parsing(format!(
                    "Failed to read PPTX archive (invalid format): {}",
                    e
                )));
            }
        };

        let slide_paths = Self::find_slide_paths(&mut archive)?;

        Ok(Self { archive, slide_paths })
    }
}

impl PptxContainer<Cursor<Vec<u8>>> {
    pub(super) fn from_bytes(data: &[u8]) -> Result<Self> {
        let cursor = Cursor::new(data.to_vec());

        let mut archive = match ZipArchive::new(cursor) {
            Ok(arc) => arc,
            Err(zip::result::ZipError::Io(io_err)) => return Err(io_err.into()), // Bubble up IO errors ~keep
            Err(e) => {
                return Err(XbergError::parsing(format!(
                    "Failed to read PPTX archive (invalid format): {}",
                    e
                )));
            }
        };

        let slide_paths = Self::find_slide_paths(&mut archive)?;

        Ok(Self { archive, slide_paths })
    }
}

impl<R: Read + Seek> PptxContainer<R> {
    pub(super) fn slide_paths(&self) -> &[String] {
        &self.slide_paths
    }

    pub(super) fn read_file(&mut self, path: &str) -> Result<Vec<u8>> {
        match self.archive.by_name(path) {
            Ok(mut file) => {
                let mut contents = Vec::new();
                // IO errors must bubble up - file read issues need user reports ~keep
                file.read_to_end(&mut contents)?;
                Ok(contents)
            }
            Err(zip::result::ZipError::FileNotFound) => {
                Err(XbergError::parsing("File not found in archive".to_string()))
            }
            Err(zip::result::ZipError::Io(io_err)) => Err(io_err.into()), // Bubble up IO errors ~keep
            Err(e) => Err(XbergError::parsing(format!("Zip error: {}", e))),
        }
    }

    pub(super) fn get_slide_rels_path(&self, slide_path: &str) -> String {
        super::image_handling::get_slide_rels_path(slide_path)
    }

    fn find_slide_paths(archive: &mut ZipArchive<R>) -> Result<Vec<String>> {
        if let Ok(rels_data) = Self::read_file_from_archive(archive, "ppt/_rels/presentation.xml.rels")
            && let Ok(paths) = super::parser::parse_presentation_rels(&rels_data)
        {
            return Ok(paths);
        }

        let mut slide_paths = Vec::new();
        for i in 0..archive.len() {
            if let Ok(file) = archive.by_index(i) {
                let name = file.name();
                if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                    slide_paths.push(name.to_string());
                }
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

    fn read_file_from_archive(archive: &mut ZipArchive<R>, path: &str) -> Result<Vec<u8>> {
        let mut file = match archive.by_name(path) {
            Ok(f) => f,
            Err(zip::result::ZipError::Io(io_err)) => return Err(io_err.into()), // Bubble up IO errors ~keep
            Err(e) => {
                return Err(XbergError::parsing(format!("Failed to read file from archive: {}", e)));
            }
        };
        let mut contents = Vec::new();
        // IO errors must bubble up - file read issues need user reports ~keep
        file.read_to_end(&mut contents)?;
        Ok(contents)
    }
}

pub(super) struct SlideIterator<R: Read + Seek> {
    container: PptxContainer<R>,
    current_index: usize,
    total_slides: usize,
}

impl<R: Read + Seek> SlideIterator<R> {
    pub(super) fn new(container: PptxContainer<R>) -> Self {
        let total_slides = container.slide_paths().len();
        Self {
            container,
            current_index: 0,
            total_slides,
        }
    }

    pub(super) fn slide_count(&self) -> usize {
        self.total_slides
    }

    /// Read and parse the next slide, skipping (and warning about) any slide
    /// whose XML part could not be read or parsed rather than aborting the
    /// whole presentation for one bad slide (#91).
    pub(super) fn next_slide(&mut self, warnings: &mut Vec<ProcessingWarning>) -> Result<Option<Slide>> {
        while self.current_index < self.total_slides {
            let slide_path = self.container.slide_paths()[self.current_index].clone();
            let slide_number = (self.current_index + 1) as u32;
            self.current_index += 1;

            let xml_data = match self.container.read_file(&slide_path) {
                Ok(data) => data,
                Err(e) => {
                    push_warning(
                        warnings,
                        "pptx",
                        format!(
                            "Could not read slide {} ('{}'): {}; slide content was not extracted",
                            slide_number, slide_path, e
                        ),
                    );
                    continue;
                }
            };

            let rels_path = self.container.get_slide_rels_path(&slide_path);
            let rels_data = self.container.read_file(&rels_path).ok();

            let mut slide = match Slide::from_xml(slide_number, &xml_data, rels_data.as_deref()) {
                Ok(slide) => slide,
                Err(e) => {
                    push_warning(
                        warnings,
                        "pptx",
                        format!(
                            "Could not parse slide {} ('{}'): {}; slide content was not extracted",
                            slide_number, slide_path, e
                        ),
                    );
                    continue;
                }
            };

            Self::resolve_graphic_frame_text(
                &mut self.container,
                &mut slide.elements,
                &slide.rel_targets,
                &slide_path,
                warnings,
            );

            return Ok(Some(slide));
        }

        Ok(None)
    }

    /// Resolve chart and SmartArt/diagram text that lives in a separate ZIP
    /// part, referenced from the slide only by relationship ID (#80). Failures
    /// to read or parse the referenced part are surfaced as warnings rather
    /// than silently dropping the chart/diagram's text.
    fn resolve_graphic_frame_text(
        container: &mut PptxContainer<R>,
        elements: &mut [SlideElement],
        rel_targets: &ahash::AHashMap<String, String>,
        slide_path: &str,
        warnings: &mut Vec<ProcessingWarning>,
    ) {
        for elem in elements.iter_mut() {
            match elem {
                SlideElement::Chart(chart_ref, _) => {
                    let Some(target) = rel_targets.get(&chart_ref.rel_id) else {
                        continue;
                    };
                    let full_path = get_full_image_path(slide_path, target);
                    match container.read_file(&full_path) {
                        Ok(bytes) => match super::parser::parse_chart_text(&bytes) {
                            Ok(text) => chart_ref.resolved_text = text,
                            Err(e) => push_warning(
                                warnings,
                                "pptx",
                                format!(
                                    "Could not parse chart part '{}': {}; chart text was not extracted",
                                    full_path, e
                                ),
                            ),
                        },
                        Err(e) => push_warning(
                            warnings,
                            "pptx",
                            format!(
                                "Could not read chart part '{}': {}; chart text was not extracted",
                                full_path, e
                            ),
                        ),
                    }
                }
                SlideElement::SmartArt(diagram_ref, _) => {
                    let Some(target) = rel_targets.get(&diagram_ref.rel_id) else {
                        continue;
                    };
                    let full_path = get_full_image_path(slide_path, target);
                    match container.read_file(&full_path) {
                        Ok(bytes) => match super::parser::parse_diagram_text(&bytes) {
                            Ok(text) => diagram_ref.resolved_text = text,
                            Err(e) => push_warning(
                                warnings,
                                "pptx",
                                format!(
                                    "Could not parse SmartArt data part '{}': {}; diagram text was not extracted",
                                    full_path, e
                                ),
                            ),
                        },
                        Err(e) => push_warning(
                            warnings,
                            "pptx",
                            format!(
                                "Could not read SmartArt data part '{}': {}; diagram text was not extracted",
                                full_path, e
                            ),
                        ),
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn get_slide_images(&mut self, slide: &Slide) -> Result<HashMap<String, Vec<u8>>> {
        let mut image_data = HashMap::new();

        for img_ref in &slide.images {
            let slide_path = &self.container.slide_paths()[slide.slide_number as usize - 1];
            let full_path = get_full_image_path(slide_path, &img_ref.target);

            if let Ok(data) = self.container.read_file(&full_path) {
                image_data.insert(img_ref.id.clone(), data);
            }
        }

        Ok(image_data)
    }
}
