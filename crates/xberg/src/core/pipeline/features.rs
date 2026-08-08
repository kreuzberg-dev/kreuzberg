//! Feature processing logic.
//!
//! This module handles feature-specific processing like chunking,
//! embedding generation, and language detection.

use crate::Result;
use crate::core::config::ExtractionConfig;
#[cfg(feature = "chunking")]
use crate::types::PageBoundary;
use crate::types::{ExtractedDocument, ProcessingWarning};
use std::borrow::Cow;

/// Recompute page boundaries against the rendered `content` string.
///
/// `PageBoundary` offsets produced during extraction are computed against raw
/// rendered/source text, but `result.content` is produced by `render_plain` which
/// trims trailing whitespace from each paragraph.  The raw page text therefore has
/// different byte lengths for pages that contain trailing-space artifacts from PDF
/// rendering.  This function re-derives the boundaries by locating each page's
/// **paragraph-normalised** content (each `"\n\n"`-separated segment trimmed, then
/// re-joined) inside the combined `content` string, so that the byte offsets passed
/// to the chunker are valid indices into `result.content`.
///
/// Pages whose content cannot be located exactly (e.g. dehyphenation, markdown
/// formatting, marker insertion, or OCR merges made the rendered text diverge from
/// `page.content`) still get a **best-effort, interpolated** boundary rather than
/// being dropped (#1294): every page is guaranteed an entry in the returned slice,
/// in page order, with non-overlapping, monotonically increasing byte ranges.
#[cfg(feature = "chunking")]
pub(crate) fn recompute_boundaries_from_pages(content: &str, pages: &[crate::types::PageContent]) -> Vec<PageBoundary> {
    if pages.is_empty() {
        return Vec::new();
    }

    let mut located = locate_page_boundaries(content, pages);
    normalize_located_boundaries(&mut located);
    fill_boundary_gaps(&mut located, pages, content);

    located.into_iter().flatten().collect()
}

/// Paragraph-normalise a page's raw content: trim each `"\n\n"`-separated segment
/// and drop empty segments, matching the rendering pipeline's paragraph trimming.
#[cfg(feature = "chunking")]
fn normalize_page_content(raw: &str) -> String {
    raw.split("\n\n")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// First pass: locate each page's content within `content`, advancing a monotonic
/// search cursor. Pages that cannot be located by either the exact-block or the
/// single-line fallback match are left as `None`, to be interpolated by
/// [`fill_boundary_gaps`].
#[cfg(feature = "chunking")]
fn locate_page_boundaries(content: &str, pages: &[crate::types::PageContent]) -> Vec<Option<PageBoundary>> {
    let mut located = Vec::with_capacity(pages.len());
    let mut search_offset = 0usize;

    for page in pages {
        if page.content.trim().is_empty() {
            located.push(Some(PageBoundary {
                page_number: page.page_number,
                byte_start: search_offset,
                byte_end: search_offset,
            }));
            continue;
        }

        let normalized = normalize_page_content(&page.content);

        if let Some(boundary) = locate_exact_block(content, &normalized, page.page_number, &mut search_offset) {
            located.push(Some(boundary));
            continue;
        }

        if let Some(boundary) = locate_by_first_line(content, page, &normalized, &mut search_offset) {
            located.push(Some(boundary));
            continue;
        }

        tracing::debug!(
            page = page.page_number,
            "Could not locate page content in rendered text — will interpolate boundary"
        );
        located.push(None);
    }

    located
}

/// Locate a page by an exact match of its paragraph-normalised content.
#[cfg(feature = "chunking")]
fn locate_exact_block(
    content: &str,
    normalized: &str,
    page_number: u32,
    search_offset: &mut usize,
) -> Option<PageBoundary> {
    let pos = content[*search_offset..].find(normalized)?;
    let byte_start = *search_offset + pos;
    let byte_end = content.floor_char_boundary(byte_start + normalized.len());
    *search_offset = byte_end;
    Some(PageBoundary {
        page_number,
        byte_start,
        byte_end,
    })
}

/// Fallback locate: anchor on the page's first non-blank line only.
///
/// The search cursor advances just past the matched anchor **line** — not the
/// estimated full-page length — so a bad length estimate for this page cannot
/// skip past (and thereby hide) legitimate content belonging to later pages.
/// That decoupling is what stops a single overshoot from cascading into
/// skipped boundaries for every subsequent page (#1294 root cause 2); any
/// resulting overlap between this page's estimated end and the next located
/// page's start is repaired afterwards by [`normalize_located_boundaries`].
#[cfg(feature = "chunking")]
fn locate_by_first_line(
    content: &str,
    page: &crate::types::PageContent,
    normalized: &str,
    search_offset: &mut usize,
) -> Option<PageBoundary> {
    let line = page.content.lines().find(|l| !l.trim().is_empty())?.trim();
    let pos = content[*search_offset..].find(line)?;
    let byte_start = *search_offset + pos;
    let raw_end = (byte_start + normalized.len()).min(content.len());
    let byte_end = content.floor_char_boundary(raw_end).max(byte_start);

    let safe_advance = content.floor_char_boundary((byte_start + line.len()).min(content.len()));
    *search_offset = safe_advance.max(*search_offset);

    Some(PageBoundary {
        page_number: page.page_number,
        byte_start,
        byte_end,
    })
}

/// Repair overlaps left by [`locate_by_first_line`]'s length estimate: walking
/// right-to-left, clamp each resolved boundary's `byte_end` to at most the next
/// resolved boundary's `byte_start`, so the returned set is always
/// non-overlapping (a precondition the chunker's page-boundary validation enforces).
#[cfg(feature = "chunking")]
fn normalize_located_boundaries(located: &mut [Option<PageBoundary>]) {
    let mut next_start: Option<usize> = None;

    for boundary_opt in located.iter_mut().rev() {
        if let Some(boundary) = boundary_opt.as_mut() {
            if let Some(next) = next_start {
                boundary.byte_end = boundary.byte_end.min(next);
                boundary.byte_start = boundary.byte_start.min(boundary.byte_end);
            }
            next_start = Some(boundary.byte_start);
        }
    }
}

/// Second pass: interpolate best-effort boundaries for runs of pages that could
/// not be located in [`locate_page_boundaries`], proportionally distributing the
/// byte range between the surrounding resolved boundaries (or content start/end)
/// by each page's normalised content length. This guarantees every page is
/// assigned a boundary even when rendering diverges too far from the raw page
/// text to locate exactly (#1294).
#[cfg(feature = "chunking")]
fn fill_boundary_gaps(located: &mut [Option<PageBoundary>], pages: &[crate::types::PageContent], content: &str) {
    let content_len = content.len();
    let mut i = 0;

    while i < located.len() {
        if located[i].is_some() {
            i += 1;
            continue;
        }

        let mut j = i;
        while j < located.len() && located[j].is_none() {
            j += 1;
        }

        let gap_start = if i == 0 {
            0
        } else {
            located[i - 1].as_ref().map_or(0, |b| b.byte_end)
        };
        let gap_end = located
            .get(j)
            .and_then(|b| b.as_ref())
            .map_or(content_len, |b| b.byte_start)
            .max(gap_start);

        distribute_gap(located, &pages[i..j], i, gap_start, gap_end, content);
        i = j;
    }
}

/// Distribute `[gap_start, gap_end)` across `run_pages` (starting at
/// `located[run_start_index]`), weighted by each page's trimmed content length.
#[cfg(feature = "chunking")]
fn distribute_gap(
    located: &mut [Option<PageBoundary>],
    run_pages: &[crate::types::PageContent],
    run_start_index: usize,
    gap_start: usize,
    gap_end: usize,
    content: &str,
) {
    let weights: Vec<usize> = run_pages.iter().map(|p| p.content.trim().len().max(1)).collect();
    let total: usize = weights.iter().sum();
    let span = gap_end - gap_start;
    let last = weights.len().saturating_sub(1);

    let mut offset = gap_start;
    for (k, weight) in weights.iter().enumerate() {
        let raw_end = if k == last {
            gap_end
        } else {
            offset + (span * weight / total)
        };
        let byte_start = content.floor_char_boundary(offset.min(content.len()));
        let byte_end = content.floor_char_boundary(raw_end.clamp(byte_start, gap_end).min(content.len()));

        located[run_start_index + k] = Some(PageBoundary {
            page_number: run_pages[k].page_number,
            byte_start,
            byte_end,
        });
        offset = byte_end;
    }
}

/// Clamp page boundaries into valid char boundaries within `text`.
///
/// `byte_start`/`byte_end` are each capped at `text.len()` and snapped down to the nearest UTF-8
/// char boundary via [`str::floor_char_boundary`]. This keeps page provenance best-effort when a
/// boundary set predates the rendered text it is paired with — e.g. the raw-extractor-text offsets
/// in `metadata.pages.boundaries` used as a fallback when [`recompute_boundaries_from_pages`] cannot
/// locate a page — without tripping the chunking page-boundary validation (#1148). Boundaries that
/// are already in range and aligned are returned unchanged.
#[cfg(feature = "chunking")]
pub(crate) fn clamp_boundaries_to_text(boundaries: &[PageBoundary], text: &str) -> Vec<PageBoundary> {
    let len = text.len();
    boundaries
        .iter()
        .map(|b| PageBoundary {
            page_number: b.page_number,
            byte_start: text.floor_char_boundary(b.byte_start.min(len)),
            byte_end: text.floor_char_boundary(b.byte_end.min(len)),
        })
        .collect()
}

/// Classify a tree-sitter code chunk's structural role from its node types.
///
/// Inspects the top-level tree-sitter node kinds captured for the chunk and maps
/// them onto the closest [`ChunkType`](crate::types::extraction::ChunkType) variant.
/// Falls back to [`ChunkType::CodeBlock`](crate::types::extraction::ChunkType::CodeBlock)
/// when no node type matches a known structural category.
#[cfg(all(feature = "tree-sitter", feature = "chunking"))]
fn classify_code_chunk(node_types: &[String]) -> crate::types::extraction::ChunkType {
    use crate::types::extraction::ChunkType;

    let is_class = node_types.iter().any(|t| {
        matches!(
            t.as_str(),
            "class_definition"
                | "class_declaration"
                | "struct_item"
                | "struct_declaration"
                | "interface_declaration"
                | "trait_item"
                | "enum_item"
                | "enum_declaration"
        )
    });
    if is_class {
        return ChunkType::Class;
    }

    let is_module = node_types.iter().any(|t| {
        matches!(
            t.as_str(),
            "module_definition" | "module" | "namespace_declaration" | "mod_item"
        )
    });
    if is_module {
        return ChunkType::Module;
    }

    let is_function = node_types.iter().any(|t| {
        matches!(
            t.as_str(),
            "function_definition"
                | "function_declaration"
                | "function_item"
                | "method_definition"
                | "method_declaration"
        )
    });
    if is_function {
        return ChunkType::Function;
    }

    ChunkType::CodeBlock
}

/// Names of the user-facing chunking settings that `try_code_chunks` silently
/// disregards, restricted to those the caller set away from their default value.
///
/// Tree-sitter code-aware chunking always bypasses the general-purpose splitter,
/// so `max_characters`/`overlap`/`chunker_type`/`trim`/`prepend_heading_context`
/// are *always* ignored in that path — but warning about that unconditionally
/// would fire on every code-chunked document, including the common case where
/// the caller never touched chunking config and is relying on defaults. That's
/// noise, not signal (#260): only settings the caller actually moved away from
/// their default are worth surfacing.
#[cfg(all(feature = "tree-sitter", feature = "chunking"))]
fn overridden_code_chunk_settings(config: &crate::core::config::ChunkingConfig) -> Vec<&'static str> {
    let default = crate::core::config::ChunkingConfig::default();
    let mut overridden = Vec::new();

    if config.max_characters != default.max_characters {
        overridden.push("max_characters");
    }
    if config.overlap != default.overlap {
        overridden.push("overlap");
    }
    if config.chunker_type != default.chunker_type {
        overridden.push("chunker_type");
    }
    if config.trim != default.trim {
        overridden.push("trim");
    }
    if config.prepend_heading_context != default.prepend_heading_context {
        overridden.push("prepend_heading_context");
    }

    overridden
}

/// Map TSLP `CodeChunk`s directly to xberg `Chunk`s, bypassing text-splitter.
///
/// When the extraction result contains code intelligence with non-empty chunks,
/// those chunks already represent semantically meaningful code boundaries produced
/// by tree-sitter. Using text-splitter would break these boundaries.
#[cfg(all(feature = "tree-sitter", feature = "chunking"))]
fn try_code_chunks(
    result: &ExtractedDocument,
    sizing: &crate::core::config::ChunkSizing,
) -> Option<Vec<crate::types::extraction::Chunk>> {
    use crate::types::extraction::{Chunk, ChunkMetadata};
    use crate::types::metadata::{CodeMetadata, FormatMetadata};

    let FormatMetadata::Code(CodeMetadata {
        chunks: code_chunks, ..
    }) = result.metadata.format.as_ref()?
    else {
        return None;
    };

    if code_chunks.is_empty() {
        return None;
    }

    let token_counter = crate::chunking::resolve_token_counter(sizing);
    let total_chunks = code_chunks.len();
    let chunks = code_chunks
        .iter()
        .enumerate()
        .map(|(chunk_index, chunk)| {
            let token_count = token_counter.as_ref().map(|counter| counter(&chunk.text));
            Chunk {
                content: chunk.text.clone(),
                chunk_type: classify_code_chunk(&chunk.node_types),
                embedding: None,
                sparse_embedding: None,
                late_interaction: None,
                metadata: ChunkMetadata {
                    byte_start: chunk.byte_start,
                    byte_end: chunk.byte_end,
                    token_count,
                    chunk_index,
                    total_chunks,
                    first_page: None,
                    last_page: None,
                    heading_context: None,
                    heading_path: chunk.context_path.clone(),
                    image_indices: Vec::new(),
                    node_ids: Vec::new(),
                    page_spans: Vec::new(),
                    classifications: Vec::new(),
                },
            }
        })
        .collect();

    Some(chunks)
}

/// Execute chunking if configured.
///
/// `heading_source_override`, when supplied, is a pre-rendered Markdown version of the
/// document used solely to resolve heading context for `chunker_type='markdown'` when
/// the final output format is `Plain` (Markdown syntax stripped from `content` would
/// otherwise hide heading structure from the chunker). The caller computes this once,
/// early in the pipeline, from the pre-derivation document — independent of
/// `result.formatted_content`, which by the time chunking runs (last, per #213) has
/// already been consumed by `apply_output_format`.
pub(super) fn execute_chunking(
    result: &mut ExtractedDocument,
    config: &ExtractionConfig,
    heading_source_override: Option<&str>,
) -> Result<()> {
    // Referenced only under `#[cfg(feature = "chunking")]` below; this keeps the
    // parameter warning-free on a build without that feature.
    let _ = heading_source_override;

    #[cfg(feature = "chunking")]
    if let Some(ref chunking_config) = config.chunking {
        // Synchronous stage — `entered()` is safe here because no `.await` follows.
        #[cfg(feature = "otel")]
        let _stage_span =
            crate::telemetry::spans::pipeline_stage_span(crate::telemetry::conventions::stages::CHUNKING).entered();

        #[cfg(feature = "tree-sitter")]
        if let Some(code_chunks) = try_code_chunks(result, &chunking_config.sizing) {
            result.chunks = Some(code_chunks);

            let resolved_config = chunking_config.resolve_preset();

            // Tree-sitter code-aware chunking bypasses the general-purpose splitter
            // entirely, so max_characters/overlap/chunker_type/trim/
            // prepend_heading_context are silently ignored. Surface that (#260) — but
            // only when the caller actually moved one of those settings away from its
            // default; warning on every code-chunked document (the common case, where
            // chunking config is left at defaults) would be noise, not signal.
            let overridden_settings = overridden_code_chunk_settings(&resolved_config);
            if !overridden_settings.is_empty() {
                let verb = if overridden_settings.len() == 1 { "was" } else { "were" };
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("chunking"),
                    message: Cow::Owned(format!(
                        "{} {verb} ignored: tree-sitter code intelligence produced structural \
                         (function/class) chunks instead of honoring the configured chunker",
                        overridden_settings.join("/"),
                    )),
                });
            }
            #[cfg(feature = "embeddings")]
            if let Some(ref embedding_config) = resolved_config.embedding
                && let Some(ref mut chunks) = result.chunks
                && let Err(e) = crate::embeddings::generate_embeddings_for_chunks(chunks, embedding_config)
            {
                tracing::warn!("Embedding generation failed: {e}. Check that ONNX Runtime is installed.");
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("embedding"),
                    message: Cow::Owned(e.to_string()),
                });
            }

            #[cfg(not(feature = "embeddings"))]
            if resolved_config.embedding.is_some() {
                tracing::warn!(
                    "Embedding config provided but embeddings feature is not enabled. Recompile with --features embeddings."
                );
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("embedding"),
                    message: Cow::Borrowed("Embeddings feature not enabled"),
                });
            }

            #[cfg(feature = "sparse-embeddings")]
            if let Some(ref sparse_config) = resolved_config.sparse_embedding
                && let Some(ref mut chunks) = result.chunks
                && let Err(e) = crate::chunking::vectors::generate_sparse_vectors_for_chunks(chunks, sparse_config)
            {
                tracing::warn!("Sparse-embedding generation failed: {e}. Check that ONNX Runtime is installed.");
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("sparse_embedding"),
                    message: Cow::Owned(e.to_string()),
                });
            }

            #[cfg(not(feature = "sparse-embeddings"))]
            if resolved_config.sparse_embedding.is_some() {
                tracing::warn!(
                    "Sparse-embedding config provided but sparse-embeddings feature is not enabled. Recompile with --features sparse-embeddings."
                );
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("sparse_embedding"),
                    message: Cow::Borrowed("sparse-embeddings feature not enabled"),
                });
            }

            #[cfg(feature = "late-interaction")]
            if let Some(ref late_config) = resolved_config.late_interaction
                && let Some(ref mut chunks) = result.chunks
                && let Err(e) =
                    crate::chunking::vectors::generate_late_interaction_vectors_for_chunks(chunks, late_config)
            {
                tracing::warn!("Late-interaction generation failed: {e}. Check that ONNX Runtime is installed.");
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("late_interaction"),
                    message: Cow::Owned(e.to_string()),
                });
            }

            #[cfg(not(feature = "late-interaction"))]
            if resolved_config.late_interaction.is_some() {
                tracing::warn!(
                    "Late-interaction config provided but late-interaction feature is not enabled. Recompile with --features late-interaction."
                );
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("late_interaction"),
                    message: Cow::Borrowed("late-interaction feature not enabled"),
                });
            }

            return Ok(());
        }

        let resolved_config = chunking_config.resolve_preset();
        let chunking_config = &resolved_config;

        // chunker_type='semantic' silently degrades to a structural-boundary heuristic
        // when no embedding model is configured (or the crate lacks the `embeddings`
        // feature); `tracing::warn!` alone is invisible to API/binding consumers (#258).
        if chunking_config.chunker_type == crate::chunking::ChunkerType::Semantic
            && crate::chunking::semantic::semantic_uses_structural_fallback(chunking_config)
        {
            result.processing_warnings.push(ProcessingWarning {
                source: Cow::Borrowed("chunking"),
                message: Cow::Borrowed(
                    "chunker_type='semantic' has no embedding model configured (or the crate was \
                     built without the 'embeddings' feature); falling back to a \
                     structural-boundary heuristic instead of embedding-driven topic detection",
                ),
            });
        }

        let recomputed_boundaries: Option<Vec<PageBoundary>> = result
            .pages
            .as_deref()
            .map(|pages| recompute_boundaries_from_pages(&result.content, pages));

        let page_boundaries: Option<&[PageBoundary]> = recomputed_boundaries
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| result.metadata.pages.as_ref().and_then(|ps| ps.boundaries.as_deref()));

        let formatted_boundaries: Option<Vec<PageBoundary>> =
            if config.output_format != crate::core::config::OutputFormat::Plain {
                result.formatted_content.as_deref().and_then(|formatted| {
                    result
                        .pages
                        .as_deref()
                        .map(|pages| recompute_boundaries_from_pages(formatted, pages))
                })
            } else {
                None
            };

        let (chunk_input, effective_page_boundaries, heading_source) =
            if config.output_format != crate::core::config::OutputFormat::Plain {
                match result.formatted_content.as_deref() {
                    Some(formatted) => {
                        let fmt_boundaries = formatted_boundaries.as_deref().filter(|s| !s.is_empty());
                        (formatted, fmt_boundaries, None)
                    }
                    None => (result.content.as_str(), page_boundaries, None),
                }
            } else {
                (
                    result.content.as_str(),
                    page_boundaries,
                    heading_source_override.or(result.formatted_content.as_deref()),
                )
            };

        let clamped_boundaries: Option<Vec<PageBoundary>> =
            effective_page_boundaries.map(|boundaries| clamp_boundaries_to_text(boundaries, chunk_input));
        let effective_page_boundaries = clamped_boundaries.as_deref();

        match crate::chunking::chunk_text_with_heading_source(
            chunk_input,
            chunking_config,
            effective_page_boundaries,
            heading_source,
        ) {
            Ok(chunking_result) => {
                result.chunks = Some(chunking_result.chunks);

                // `chunk_text_with_heading_source` resolves `heading_context` but never
                // derives the binding-friendly `heading_path` breadcrumb from it — only
                // `chunk_for_rag` and the code-chunk path did that (#256).
                if let Some(ref mut chunks) = result.chunks {
                    for chunk in chunks.iter_mut() {
                        chunk.metadata.heading_path =
                            crate::chunking::heading_path_from_context(&chunk.metadata.heading_context);
                    }
                }

                if let Some(ref images) = result.images
                    && let Some(ref mut chunks) = result.chunks
                {
                    // Page-addressable chunks (PDF, and any format with page boundaries):
                    // link an image when its page falls within the chunk's page range.
                    for chunk in chunks.iter_mut() {
                        if let (Some(first), Some(last)) = (chunk.metadata.first_page, chunk.metadata.last_page) {
                            chunk.metadata.image_indices = images
                                .iter()
                                .enumerate()
                                .filter_map(|(idx, img)| {
                                    let pg = img.page_number?;
                                    (pg >= first && pg <= last).then_some(idx as u32)
                                })
                                .collect();
                        }
                    }

                    // Page-less formats (DOCX/PPTX/HTML, …) carry no page number on either
                    // the chunk or its images, so the page-range match above can never link
                    // them (#256). When the whole document collapses to a single chunk,
                    // every page-less image unambiguously belongs to it — no page
                    // correlation is needed to know that. A page-less *multi*-chunk document
                    // has no reliable image-to-chunk signal today (no byte-position-aware
                    // image reference in rendered content) and is left unresolved.
                    if let [only_chunk] = chunks.as_mut_slice()
                        && only_chunk.metadata.first_page.is_none()
                        && only_chunk.metadata.last_page.is_none()
                    {
                        only_chunk.metadata.image_indices = images
                            .iter()
                            .enumerate()
                            .filter_map(|(idx, img)| img.page_number.is_none().then_some(idx as u32))
                            .collect();
                    }
                }

                if let Some(ref structure) = result.document
                    && let Some(ref mut chunks) = result.chunks
                {
                    crate::chunking::page_spans::populate_page_span_bboxes(chunks, structure);
                }

                #[cfg(feature = "embeddings")]
                if let Some(ref embedding_config) = chunking_config.embedding
                    && let Some(ref mut chunks) = result.chunks
                    && let Err(e) = crate::embeddings::generate_embeddings_for_chunks(chunks, embedding_config)
                {
                    tracing::warn!("Embedding generation failed: {e}. Check that ONNX Runtime is installed.");
                    result.processing_warnings.push(ProcessingWarning {
                        source: Cow::Borrowed("embedding"),
                        message: Cow::Owned(e.to_string()),
                    });
                }

                #[cfg(not(feature = "embeddings"))]
                if chunking_config.embedding.is_some() {
                    tracing::warn!(
                        "Embedding config provided but embeddings feature is not enabled. Recompile with --features embeddings."
                    );
                    result.processing_warnings.push(ProcessingWarning {
                        source: Cow::Borrowed("embedding"),
                        message: Cow::Borrowed("Embeddings feature not enabled"),
                    });
                }

                #[cfg(feature = "sparse-embeddings")]
                if let Some(ref sparse_config) = chunking_config.sparse_embedding
                    && let Some(ref mut chunks) = result.chunks
                    && let Err(e) = crate::chunking::vectors::generate_sparse_vectors_for_chunks(chunks, sparse_config)
                {
                    tracing::warn!("Sparse-embedding generation failed: {e}. Check that ONNX Runtime is installed.");
                    result.processing_warnings.push(ProcessingWarning {
                        source: Cow::Borrowed("sparse_embedding"),
                        message: Cow::Owned(e.to_string()),
                    });
                }

                #[cfg(not(feature = "sparse-embeddings"))]
                if chunking_config.sparse_embedding.is_some() {
                    tracing::warn!(
                        "Sparse-embedding config provided but sparse-embeddings feature is not enabled. Recompile with --features sparse-embeddings."
                    );
                    result.processing_warnings.push(ProcessingWarning {
                        source: Cow::Borrowed("sparse_embedding"),
                        message: Cow::Borrowed("sparse-embeddings feature not enabled"),
                    });
                }

                #[cfg(feature = "late-interaction")]
                if let Some(ref late_config) = chunking_config.late_interaction
                    && let Some(ref mut chunks) = result.chunks
                    && let Err(e) =
                        crate::chunking::vectors::generate_late_interaction_vectors_for_chunks(chunks, late_config)
                {
                    tracing::warn!("Late-interaction generation failed: {e}. Check that ONNX Runtime is installed.");
                    result.processing_warnings.push(ProcessingWarning {
                        source: Cow::Borrowed("late_interaction"),
                        message: Cow::Owned(e.to_string()),
                    });
                }

                #[cfg(not(feature = "late-interaction"))]
                if chunking_config.late_interaction.is_some() {
                    tracing::warn!(
                        "Late-interaction config provided but late-interaction feature is not enabled. Recompile with --features late-interaction."
                    );
                    result.processing_warnings.push(ProcessingWarning {
                        source: Cow::Borrowed("late_interaction"),
                        message: Cow::Borrowed("late-interaction feature not enabled"),
                    });
                }
            }
            Err(e) => {
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("chunking"),
                    message: Cow::Owned(e.to_string()),
                });
            }
        }
    }

    #[cfg(not(feature = "chunking"))]
    if config.chunking.is_some() {
        result.processing_warnings.push(ProcessingWarning {
            source: Cow::Borrowed("chunking"),
            message: Cow::Borrowed("Chunking feature not enabled"),
        });
    }

    Ok(())
}

/// Execute language detection if configured.
pub(super) fn execute_language_detection(result: &mut ExtractedDocument, config: &ExtractionConfig) -> Result<()> {
    #[cfg(feature = "language-detection")]
    if let Some(ref lang_config) = config.language_detection {
        // Synchronous stage — `entered()` is safe here because no `.await` follows.
        #[cfg(feature = "otel")]
        let _stage_span =
            crate::telemetry::spans::pipeline_stage_span(crate::telemetry::conventions::stages::LANGUAGE_DETECTION)
                .entered();

        match crate::language_detection::detect_languages(&result.content, lang_config) {
            Ok(detected) => {
                result.detected_languages = detected;
            }
            Err(e) => {
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("language_detection"),
                    message: Cow::Owned(e.to_string()),
                });
            }
        }
    }

    #[cfg(not(feature = "language-detection"))]
    if config.language_detection.is_some() {
        result.processing_warnings.push(ProcessingWarning {
            source: Cow::Borrowed("language_detection"),
            message: Cow::Borrowed("Language detection feature not enabled"),
        });
    }

    Ok(())
}

/// Execute token reduction if configured.
pub(super) fn execute_token_reduction(result: &mut ExtractedDocument, config: &ExtractionConfig) -> Result<()> {
    #[cfg(feature = "quality")]
    if let Some(ref tr_config) = config.token_reduction {
        let level = crate::text::token_reduction::ReductionLevel::from(tr_config.mode.as_str());

        if !matches!(level, crate::text::token_reduction::ReductionLevel::Off) {
            // Synchronous stage — `entered()` is safe here because no `.await` follows.
            #[cfg(feature = "otel")]
            let _stage_span =
                crate::telemetry::spans::pipeline_stage_span(crate::telemetry::conventions::stages::TOKEN_REDUCTION)
                    .entered();

            let impl_config = crate::text::token_reduction::TokenReductionConfig {
                level,
                ..Default::default()
            };

            let lang_owned = result
                .detected_languages
                .as_deref()
                .and_then(|langs| langs.first().cloned());
            let lang_hint: Option<&str> = lang_owned.as_deref();

            let mut warnings: Vec<String> = Vec::new();

            match crate::text::token_reduction::reduce_tokens(&result.content, &impl_config, lang_hint) {
                Ok(reduced) => result.content = reduced,
                Err(e) => warnings.push(e.to_string()),
            }
            if let Some(formatted) = result.formatted_content.as_deref() {
                match crate::text::token_reduction::reduce_tokens(formatted, &impl_config, lang_hint) {
                    Ok(reduced) => result.formatted_content = Some(reduced),
                    Err(e) => warnings.push(e.to_string()),
                }
            }

            for message in warnings {
                result.processing_warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("token_reduction"),
                    message: Cow::Owned(message),
                });
            }
        }
    }

    #[cfg(not(feature = "quality"))]
    if config.token_reduction.is_some() {
        result.processing_warnings.push(ProcessingWarning {
            source: Cow::Borrowed("token_reduction"),
            message: Cow::Borrowed("Token reduction requires the quality feature"),
        });
    }

    Ok(())
}

#[cfg(all(test, feature = "chunking"))]
mod tests {
    use super::*;
    use crate::types::PageContent;

    fn make_page(page_number: u32, content: impl Into<String>) -> PageContent {
        PageContent {
            page_number,
            content: content.into(),
            tables: vec![],
            image_indices: vec![],
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            section_name: None,
            speaker_notes: None,
            sheet_name: None,
        }
    }

    #[test]
    fn recompute_boundaries_exact_match_produces_full_boundary_set() {
        let p1 = "Hello world";
        let p2 = "Second page text";
        let p3 = "Third page here";
        let content = format!("{p1}\n\n{p2}\n\n{p3}");

        let pages = vec![make_page(1, p1), make_page(2, p2), make_page(3, p3)];
        let boundaries = recompute_boundaries_from_pages(&content, &pages);

        assert_eq!(boundaries.len(), 3, "all pages should resolve to boundaries");
        assert_eq!(&content[boundaries[0].byte_start..boundaries[0].byte_end], p1);
        assert_eq!(&content[boundaries[1].byte_start..boundaries[1].byte_end], p2);
        assert_eq!(&content[boundaries[2].byte_start..boundaries[2].byte_end], p3);
    }

    #[test]
    fn recompute_boundaries_raw_content_causes_interpolated_page() {
        let p1_clean = "Hello world";
        let p2_raw = "ab\x01cd";
        let p2_clean = "ab-cd";
        let p3_clean = "Third page";
        let content = format!("{p1_clean}\n\n{p2_clean}\n\n{p3_clean}");

        let pages = vec![make_page(1, p1_clean), make_page(2, p2_raw), make_page(3, p3_clean)];
        let boundaries = recompute_boundaries_from_pages(&content, &pages);

        assert_eq!(
            boundaries.len(),
            3,
            "every page must get a boundary, including unlocatable ones"
        );
        assert_eq!(boundaries[0].page_number, 1);
        assert_eq!(
            boundaries[1].page_number, 2,
            "unlocatable page 2 must be interpolated, not skipped"
        );
        assert_eq!(boundaries[2].page_number, 3);

        for w in boundaries.windows(2) {
            assert!(
                w[0].byte_end <= w[1].byte_start,
                "boundaries must be non-overlapping: {:?} then {:?}",
                w[0],
                w[1]
            );
        }
        assert!(boundaries[1].byte_start <= boundaries[1].byte_end);
        assert!(boundaries[1].byte_end <= content.len());
        assert!(boundaries[1].byte_start >= boundaries[0].byte_end);
        assert!(boundaries[1].byte_end <= boundaries[2].byte_start);
    }

    #[test]
    fn recompute_boundaries_fallback_length_overshoot_does_not_cascade() {
        let page_a_raw = "Start marker\n\nExtra padding text that never appears in the final rendering";
        let content = "Start marker\n\nNext page text";

        let pages = vec![make_page(1, page_a_raw), make_page(2, "Next page text")];
        let boundaries = recompute_boundaries_from_pages(content, &pages);

        assert_eq!(
            boundaries.len(),
            2,
            "both pages must resolve; overshoot must not skip page 2"
        );
        assert_eq!(boundaries[0].page_number, 1);
        assert_eq!(boundaries[1].page_number, 2);
        assert!(
            boundaries[0].byte_end <= boundaries[1].byte_start,
            "overshot page 1 end ({}) must be clamped below page 2 start ({})",
            boundaries[0].byte_end,
            boundaries[1].byte_start
        );
        assert_eq!(
            &content[boundaries[1].byte_start..boundaries[1].byte_end],
            "Next page text",
            "page 2 must resolve via exact match once the search cursor isn't overshot"
        );
    }

    #[test]
    fn recompute_boundaries_cleaned_content_resolves_all_pages() {
        let p1_clean = "Hello world";
        let p2_clean = "ab-cd";
        let p3_clean = "Third page";
        let content = format!("{p1_clean}\n\n{p2_clean}\n\n{p3_clean}");

        let pages = vec![make_page(1, p1_clean), make_page(2, p2_clean), make_page(3, p3_clean)];
        let boundaries = recompute_boundaries_from_pages(&content, &pages);

        assert_eq!(boundaries.len(), 3, "all pages should resolve after fix");
        assert_eq!(&content[boundaries[1].byte_start..boundaries[1].byte_end], p2_clean);
    }

    #[test]
    fn recompute_boundaries_trailing_space_pages_all_resolve() {
        let p1_raw = "Heading \n\nBody paragraph one. ";
        let p2_raw = "Second heading \n\nBody paragraph two. ";
        let p3_raw = "Conclusion. ";

        let p1_norm = "Heading\n\nBody paragraph one.";
        let p2_norm = "Second heading\n\nBody paragraph two.";
        let p3_norm = "Conclusion.";
        let content = format!("{p1_norm}\n\n{p2_norm}\n\n{p3_norm}");

        let pages = vec![make_page(1, p1_raw), make_page(2, p2_raw), make_page(3, p3_raw)];
        let boundaries = recompute_boundaries_from_pages(&content, &pages);

        assert_eq!(boundaries.len(), 3, "all pages must resolve despite trailing spaces");
        assert_eq!(&content[boundaries[0].byte_start..boundaries[0].byte_end], p1_norm);
        assert_eq!(&content[boundaries[1].byte_start..boundaries[1].byte_end], p2_norm);
        assert_eq!(&content[boundaries[2].byte_start..boundaries[2].byte_end], p3_norm);
    }

    #[test]
    fn recompute_boundaries_after_ocr_fills_scanned_pdf() {
        let p1_ocr = "Invoice\n\nBill To: Acme Corp";
        let p2_ocr = "Line items\n\nProduct A  $100.00";
        let p3_ocr = "Total: $100.00";

        let pages = vec![make_page(1, p1_ocr), make_page(2, p2_ocr), make_page(3, p3_ocr)];

        let combined: String = pages
            .iter()
            .filter(|p| !p.content.trim().is_empty())
            .map(|p| p.content.trim())
            .collect::<Vec<_>>()
            .join("\n\n");

        let boundaries = recompute_boundaries_from_pages(&combined, &pages);

        assert_eq!(boundaries.len(), 3, "all OCR-filled pages should resolve to boundaries");

        for b in &boundaries {
            assert!(
                b.byte_start <= b.byte_end,
                "page {} boundary start ({}) must not exceed end ({})",
                b.page_number,
                b.byte_start,
                b.byte_end
            );
            assert!(
                b.byte_end <= combined.len(),
                "page {} byte_end ({}) exceeds combined content length ({})",
                b.page_number,
                b.byte_end,
                combined.len()
            );
        }

        let p1 = &boundaries[0];
        assert!(
            combined[p1.byte_start..p1.byte_end].contains("Invoice"),
            "page 1 boundary should cover the OCR text starting with 'Invoice'"
        );

        let p3 = &boundaries[2];
        assert!(
            combined[p3.byte_start..p3.byte_end].contains("Total"),
            "page 3 boundary should cover the OCR text containing 'Total'"
        );
    }

    fn make_result_with_formatted(plain: &str, formatted: &str) -> ExtractedDocument {
        ExtractedDocument {
            content: plain.to_string(),
            formatted_content: Some(formatted.to_string()),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        }
    }

    fn make_result_with_pages_and_formatted(
        plain: &str,
        formatted: &str,
        pages: Vec<crate::types::PageContent>,
    ) -> ExtractedDocument {
        ExtractedDocument {
            content: plain.to_string(),
            formatted_content: Some(formatted.to_string()),
            pages: Some(pages),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        }
    }

    fn markdown_chunking_config() -> crate::core::config::ExtractionConfig {
        crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Markdown,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn chunks_content_is_markdown_when_output_format_is_markdown() {
        let plain = "SH-001 Luca Bianchi Common Germany 3500000\nSH-002 Jeni Doe Common Singapore 2800000";
        let markdown = "| SH-001 | Luca Bianchi | Common | Germany | 3,500,000 |\n\
                        | SH-002 | Jeni Doe | Common | Singapore | 2,800,000 |";

        let config = markdown_chunking_config();
        let mut result = make_result_with_formatted(plain, markdown);

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                chunk.content.contains('|'),
                "chunk content must be markdown (contain '|'), got: {:?}",
                chunk.content
            );
        }
        for chunk in &chunks {
            assert!(
                !chunk.content.starts_with("SH-001 Luca"),
                "chunk content must not be plain text, got: {:?}",
                chunk.content
            );
        }
        assert!(
            result.formatted_content.is_some(),
            "formatted_content must not be consumed by chunking"
        );
    }

    #[test]
    fn chunks_content_is_plain_when_output_format_is_plain() {
        let plain = "# Heading\n\nRow one content\nRow two content";
        let heading_source = "# Heading\n\nRow one content\nRow two content";

        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Plain,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Markdown,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: plain.to_string(),
            formatted_content: Some(heading_source.to_string()),
            mime_type: std::borrow::Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty());
        let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join(" ");
        assert!(
            all_content.contains("Row one content") || all_content.contains("Heading"),
            "plain-mode chunks must contain source text, got: {:?}",
            all_content
        );
        assert!(
            result.formatted_content.is_some(),
            "Plain path must not consume formatted_content"
        );
    }

    #[test]
    fn chunks_content_matches_when_no_formatted_content_and_markdown_format() {
        let plain = "Some plain text without markdown pre-render";

        let config = markdown_chunking_config();
        let mut result = ExtractedDocument {
            content: plain.to_string(),
            formatted_content: None,
            mime_type: std::borrow::Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty());
        assert_eq!(chunks[0].content, plain);
    }

    #[test]
    fn chunks_content_uses_formatted_content_for_djot_output_format() {
        let plain = "row one data\nrow two data";
        let djot = "{row one | data}\n{row two | data}";

        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Djot,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = make_result_with_formatted(plain, djot);

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty());
        let all_content: String = chunks.iter().map(|c| c.content.as_str()).collect::<Vec<_>>().join("\n");
        assert!(
            all_content.contains('{'),
            "chunk content must use djot formatted_content, got: {:?}",
            all_content
        );
        assert!(
            !all_content.starts_with("row one data\nrow two"),
            "chunk content must not be plain text, got: {:?}",
            all_content
        );
    }

    #[test]
    fn chunk_page_metadata_is_none_when_pages_field_absent() {
        let plain = "Page one content\n\nPage two content";
        let markdown = "# Page one\n\nPage one content\n\n# Page two\n\nPage two content";

        let config = markdown_chunking_config();
        let mut result = make_result_with_formatted(plain, markdown);

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                chunk.metadata.first_page.is_none(),
                "first_page must be None when result.pages is absent, got: {:?}",
                chunk.metadata.first_page
            );
        }
    }

    #[test]
    fn chunk_page_provenance_present_for_markdown_output_with_pages() {
        let p1 = "Introduction text for page one";
        let p2 = "Conclusion text for page two";
        let plain = format!("{p1}\n\n{p2}");
        let markdown = format!("# Introduction\n\n{p1}\n\n# Conclusion\n\n{p2}");

        let pages = vec![make_page(1, p1), make_page(2, p2)];
        let config = markdown_chunking_config();
        let mut result = make_result_with_pages_and_formatted(&plain, &markdown, pages);

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty(), "chunks must be non-empty");
        let has_provenance = chunks.iter().any(|c| c.metadata.first_page.is_some());
        assert!(
            has_provenance,
            "at least one chunk must carry first_page when result.pages is populated and output_format=Markdown"
        );
    }

    #[test]
    fn chunk_page_provenance_single_page_markdown_output() {
        let p1 = "Single page content for the document";
        let markdown = format!("# Document\n\n{p1}");

        let pages = vec![make_page(1, p1)];
        let config = markdown_chunking_config();
        let mut result = ExtractedDocument {
            content: p1.to_string(),
            formatted_content: Some(markdown),
            pages: Some(pages),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result
            .chunks
            .expect("chunks must be Some(...) — not null — for single-page with chunking configured");
        assert!(!chunks.is_empty(), "chunks must be non-empty when content is present");
        let has_page_one = chunks.iter().any(|c| c.metadata.first_page == Some(1));
        assert!(
            has_page_one,
            "single-page chunk must have first_page = Some(1) for markdown output, got: {:?}",
            chunks.iter().map(|c| c.metadata.first_page).collect::<Vec<_>>()
        );
    }

    #[test]
    fn chunk_page_provenance_plain_output_unaffected_by_formatted_boundaries() {
        let p1 = "First page text";
        let p2 = "Second page text";
        let plain = format!("{p1}\n\n{p2}");
        let heading_source = format!("# Doc\n\n{p1}\n\n# End\n\n{p2}");

        let pages = vec![make_page(1, p1), make_page(2, p2)];
        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Plain,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Markdown,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: plain.clone(),
            formatted_content: Some(heading_source),
            pages: Some(pages),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(
                !chunk.content.contains("# Doc"),
                "plain-output chunks must not contain markdown heading syntax"
            );
        }
        let has_provenance = chunks.iter().any(|c| c.metadata.first_page.is_some());
        assert!(
            has_provenance,
            "plain-output chunks must carry page provenance when result.pages is set"
        );
    }

    #[test]
    fn chunk_page_provenance_html_output_ascii_content() {
        let p1 = "Introduction section content";
        let p2 = "Conclusion section content";
        let plain = format!("{p1}\n\n{p2}");
        let html = format!("<p>{p1}</p>\n<p>{p2}</p>");

        let pages = vec![make_page(1, p1), make_page(2, p2)];
        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Html,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: plain,
            formatted_content: Some(html),
            pages: Some(pages),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated for HTML output");
        assert!(!chunks.is_empty());
        let has_provenance = chunks.iter().any(|c| c.metadata.first_page.is_some());
        assert!(
            has_provenance,
            "HTML output with ASCII page text must carry page provenance; got: {:?}",
            chunks.iter().map(|c| c.metadata.first_page).collect::<Vec<_>>()
        );
    }

    #[test]
    fn chunk_page_provenance_html_output_recovers_via_interpolation_for_html_special_chars() {
        let p1_raw = "AT&T quarterly report";
        let plain = p1_raw.to_string();
        let html = "<p>AT&amp;T quarterly report</p>".to_string();

        let pages = vec![make_page(1, p1_raw)];
        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Html,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: plain,
            formatted_content: Some(html),
            pages: Some(pages),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must still be produced");
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert_eq!(
                chunk.metadata.first_page,
                Some(1),
                "single un-locatable page must still be interpolated to page 1, got: {:?}",
                chunk.metadata.first_page
            );
            assert_eq!(chunk.metadata.last_page, Some(1));
        }
    }

    #[test]
    fn chunk_page_provenance_djot_output_with_pages() {
        let p1 = "Djot page one text";
        let p2 = "Djot page two text";
        let plain = format!("{p1}\n\n{p2}");
        let djot = format!("# Section One\n\n{p1}\n\n# Section Two\n\n{p2}");

        let pages = vec![make_page(1, p1), make_page(2, p2)];
        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Djot,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: plain,
            formatted_content: Some(djot),
            pages: Some(pages),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated for Djot output");
        assert!(!chunks.is_empty());
        let has_provenance = chunks.iter().any(|c| c.metadata.first_page.is_some());
        assert!(
            has_provenance,
            "Djot output must carry page provenance when result.pages is populated"
        );
    }

    #[test]
    fn chunk_page_provenance_multi_chunk_single_page() {
        let p1 = "Alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi";
        let plain = p1.to_string();
        let markdown = format!("# Doc\n\n{p1}");

        let pages = vec![make_page(1, p1)];
        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 20,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: plain,
            formatted_content: Some(markdown),
            pages: Some(pages),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(chunks.len() > 1, "small cap must produce multiple chunks");
        for chunk in &chunks {
            if chunk.metadata.first_page.is_some() {
                assert_eq!(
                    chunk.metadata.first_page,
                    Some(1),
                    "all chunks of a single-page document must have first_page = Some(1)"
                );
                assert_eq!(
                    chunk.metadata.last_page,
                    Some(1),
                    "all chunks of a single-page document must have last_page = Some(1)"
                );
            }
        }
        let attributed = chunks.iter().filter(|c| c.metadata.first_page.is_some()).count();
        assert!(attributed > 0, "at least one chunk must be attributed to page 1");
    }

    fn plain_chunking_config() -> crate::core::config::ExtractionConfig {
        crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Plain,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn chunk_page_provenance_single_page_plain_output() {
        let p1 = "Single page plain text content for the document";
        let config = plain_chunking_config();
        let mut result = ExtractedDocument {
            content: p1.to_string(),
            pages: Some(vec![make_page(1, p1)]),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be Some for plain single-page");
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert_eq!(chunk.metadata.first_page, Some(1));
            assert_eq!(chunk.metadata.last_page, Some(1));
        }
    }

    #[test]
    fn chunk_page_provenance_single_page_plain_output_content_empty_produces_empty_chunks() {
        let config = plain_chunking_config();
        let mut result = ExtractedDocument {
            content: String::new(),
            pages: Some(vec![make_page(1, "")]),
            mime_type: std::borrow::Cow::Borrowed("application/pdf"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result
            .chunks
            .expect("chunks must be Some([]) not None for empty content");
        assert!(chunks.is_empty());
    }

    #[test]
    #[cfg(feature = "chunking")]
    fn clamp_boundaries_to_text_caps_stale_offsets_within_text() {
        use crate::chunking::validation::validate_utf8_boundaries;

        let text = "rendered content that is shorter than the raw extractor text";
        let stale = [PageBoundary {
            page_number: 1,
            byte_start: 0,
            byte_end: text.len() + 926,
        }];
        assert!(validate_utf8_boundaries(text, &stale).is_err());

        let clamped = clamp_boundaries_to_text(&stale, text);
        assert_eq!(clamped[0].byte_start, 0);
        assert_eq!(clamped[0].byte_end, text.len());
        assert!(validate_utf8_boundaries(text, &clamped).is_ok());

        let valid = [PageBoundary {
            page_number: 2,
            byte_start: 0,
            byte_end: 10,
        }];
        let unchanged = clamp_boundaries_to_text(&valid, text);
        assert_eq!(unchanged[0].byte_start, 0);
        assert_eq!(unchanged[0].byte_end, 10);
        assert_eq!(unchanged[0].page_number, 2);

        let multibyte = "héllo";
        let mid = [PageBoundary {
            page_number: 1,
            byte_start: 0,
            byte_end: 100,
        }];
        let mb = clamp_boundaries_to_text(&mid, multibyte);
        assert_eq!(mb[0].byte_end, multibyte.len());
        assert!(multibyte.is_char_boundary(mb[0].byte_end));
    }

    /// Regression test for #258: `chunker_type='semantic'` without an embedding model
    /// (the default in this feature build, and always the case without the
    /// `embeddings` feature) silently falls back to a structural-boundary heuristic.
    /// `tracing::warn!` alone is invisible to API/binding consumers, so a real
    /// `ProcessingWarning` must be pushed.
    #[test]
    fn semantic_chunker_fallback_pushes_processing_warning() {
        let config = crate::core::config::ExtractionConfig {
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 500,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Semantic,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: "Some content to chunk semantically.".to_string(),
            mime_type: std::borrow::Cow::Borrowed("text/plain"),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        assert!(
            result
                .processing_warnings
                .iter()
                .any(|w| w.source == "chunking" && w.message.contains("structural-boundary heuristic")),
            "expected a 'chunking' ProcessingWarning about the structural-boundary fallback, got: {:?}",
            result.processing_warnings
        );
    }

    /// Regression test for #256: `chunk_text_with_heading_source` resolves
    /// `heading_context` per chunk, but `execute_chunking` never derived the
    /// binding-friendly `heading_path` breadcrumb from it.
    #[test]
    fn heading_path_derived_from_heading_context_after_chunking() {
        let markdown = "# Title\n\nIntro paragraph text.\n\n## Section\n\nSection body text here.";
        let config = crate::core::config::ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 40,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Markdown,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = make_result_with_formatted(markdown, markdown);

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(!chunks.is_empty());
        let has_populated_path = chunks
            .iter()
            .any(|c| c.metadata.heading_context.is_some() && !c.metadata.heading_path.is_empty());
        assert!(
            has_populated_path,
            "at least one chunk under a heading must have a non-empty heading_path, got: {:?}",
            chunks.iter().map(|c| &c.metadata.heading_path).collect::<Vec<_>>()
        );
        for chunk in &chunks {
            let expected: Vec<String> = chunk
                .metadata
                .heading_context
                .as_ref()
                .map(|ctx| ctx.headings.iter().map(|h| h.text.clone()).collect())
                .unwrap_or_default();
            assert_eq!(
                chunk.metadata.heading_path, expected,
                "heading_path must equal heading_context.headings[].text in order"
            );
        }
    }

    /// Regression test for #256: page-less formats (DOCX/PPTX/HTML, simulated here by a
    /// document with no `pages`) never linked images to chunks, because the image-index
    /// filter required `img.page_number` to be `Some(_)` unconditionally. When the whole
    /// document collapses to a single page-less chunk, every page-less image
    /// unambiguously belongs to it.
    #[test]
    fn image_indices_populated_for_pageless_single_chunk_document() {
        use crate::types::ExtractedImage;

        let config = crate::core::config::ExtractionConfig {
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 2000,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: "A short document with one embedded image.".to_string(),
            mime_type: std::borrow::Cow::Borrowed(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            ),
            images: Some(vec![ExtractedImage {
                page_number: None,
                ..Default::default()
            }]),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert_eq!(
            chunks.len(),
            1,
            "expected the whole short document to be a single chunk"
        );
        assert_eq!(chunks[0].metadata.first_page, None);
        assert_eq!(chunks[0].metadata.last_page, None);
        assert_eq!(
            chunks[0].metadata.image_indices,
            vec![0u32],
            "the single page-less chunk must link the single page-less image"
        );
    }

    /// A multi-chunk page-less document has no reliable per-chunk image signal today;
    /// image_indices must stay empty rather than guessing (documented residual gap).
    #[test]
    fn image_indices_stay_empty_for_pageless_multi_chunk_document() {
        use crate::types::ExtractedImage;

        let config = crate::core::config::ExtractionConfig {
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 20,
                overlap: 0,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = ExtractedDocument {
            content: "First chunk text here. Second chunk text here. Third chunk text here.".to_string(),
            mime_type: std::borrow::Cow::Borrowed("text/html"),
            images: Some(vec![ExtractedImage {
                page_number: None,
                ..Default::default()
            }]),
            ..Default::default()
        };

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("chunks must be populated");
        assert!(chunks.len() > 1, "expected multiple chunks, got {}", chunks.len());
        for chunk in &chunks {
            assert!(
                chunk.metadata.image_indices.is_empty(),
                "multi-chunk page-less documents have no reliable image signal yet"
            );
        }
    }

    /// Builds an `ExtractedDocument` carrying a single tree-sitter code chunk, so
    /// `try_code_chunks` takes the code-aware path in `execute_chunking`.
    #[cfg(feature = "tree-sitter")]
    fn make_code_result() -> ExtractedDocument {
        use crate::types::metadata::{CodeChunkInfo, CodeMetadata, FormatMetadata};

        let mut result = ExtractedDocument {
            content: "fn main() {}".to_string(),
            mime_type: std::borrow::Cow::Borrowed("text/x-rust"),
            ..Default::default()
        };
        result.metadata.format = Some(FormatMetadata::Code(CodeMetadata {
            chunks: vec![CodeChunkInfo {
                text: "fn main() {}".to_string(),
                context_path: vec![],
                node_types: vec!["function_item".to_string()],
                byte_start: 0,
                byte_end: 12,
            }],
            data: None,
        }));
        result
    }

    /// Regression test for #260: tree-sitter code-aware chunking silently bypasses the
    /// user's `max_characters`/`overlap`/`chunker_type`/`trim`/`prepend_heading_context`
    /// with no indication anything was overridden. When the caller has actually moved
    /// settings away from their defaults, the exact overridden names must be surfaced.
    #[cfg(feature = "tree-sitter")]
    #[test]
    fn code_chunking_override_pushes_processing_warning_naming_overridden_settings() {
        let config = crate::core::config::ExtractionConfig {
            chunking: Some(crate::core::config::ChunkingConfig {
                max_characters: 20,
                overlap: 5,
                trim: true,
                chunker_type: crate::chunking::ChunkerType::Text,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut result = make_code_result();

        execute_chunking(&mut result, &config, None).unwrap();

        assert!(result.chunks.is_some(), "code chunks must still be produced");
        let warning = result
            .processing_warnings
            .iter()
            .find(|w| w.source == "chunking")
            .unwrap_or_else(|| {
                panic!(
                    "expected a 'chunking' ProcessingWarning, got: {:?}",
                    result.processing_warnings
                )
            });
        assert_eq!(warning.source, "chunking");
        assert_eq!(
            warning.message,
            "max_characters/overlap were ignored: tree-sitter code intelligence produced \
             structural (function/class) chunks instead of honoring the configured chunker",
            "warning must name exactly the settings the caller overrode (trim=true and \
             chunker_type=Text are both defaults here, so must not be listed)"
        );
    }

    /// Regression test for #260 scoping: when the caller leaves chunking config at its
    /// defaults, tree-sitter code-aware chunking must NOT push an override warning — every
    /// code-chunked document would otherwise get a warning that is never actionable, which
    /// is noise rather than signal.
    #[cfg(feature = "tree-sitter")]
    #[test]
    fn code_chunking_with_default_settings_pushes_no_override_warning() {
        let config = crate::core::config::ExtractionConfig {
            chunking: Some(crate::core::config::ChunkingConfig::default()),
            ..Default::default()
        };
        let mut result = make_code_result();

        execute_chunking(&mut result, &config, None).unwrap();

        assert!(result.chunks.is_some(), "code chunks must still be produced");
        assert_eq!(
            result.processing_warnings.len(),
            0,
            "no ProcessingWarning must be pushed when chunking config is left at defaults, got: {:?}",
            result.processing_warnings
        );
    }

    /// Regression test for #255: the code-chunk path (`try_code_chunks`) always set
    /// `token_count: None`, even with `ChunkSizing::Tokenizer` configured.
    #[cfg(all(feature = "tree-sitter", feature = "chunking-tokenizers"))]
    #[test]
    fn code_chunk_token_count_populated_from_registered_tokenizer_backend() {
        use crate::plugins::registry::test_support::TokenizerRegistryGuard;
        use crate::plugins::{Plugin, TokenizerBackend, register_tokenizer_backend};
        use crate::types::metadata::{CodeChunkInfo, CodeMetadata, FormatMetadata};
        use std::sync::Arc;

        struct WordCountTokenizer;
        impl Plugin for WordCountTokenizer {
            fn name(&self) -> &str {
                "features-code-chunk-word-count-tokenizer"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }
        impl TokenizerBackend for WordCountTokenizer {
            fn count_tokens(&self, text: &str) -> usize {
                text.split_whitespace().count()
            }
        }

        let _guard = TokenizerRegistryGuard::acquire();
        register_tokenizer_backend(Arc::new(WordCountTokenizer)).unwrap();

        let config = crate::core::config::ExtractionConfig {
            chunking: Some(crate::core::config::ChunkingConfig {
                sizing: crate::core::config::ChunkSizing::Tokenizer {
                    model: "features-code-chunk-word-count-tokenizer".to_string(),
                    cache_dir: None,
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let code_text = "fn add(a: i32, b: i32) -> i32 { a + b }";
        let mut result = ExtractedDocument {
            content: code_text.to_string(),
            mime_type: std::borrow::Cow::Borrowed("text/x-rust"),
            ..Default::default()
        };
        result.metadata.format = Some(FormatMetadata::Code(CodeMetadata {
            chunks: vec![CodeChunkInfo {
                text: code_text.to_string(),
                context_path: vec![],
                node_types: vec!["function_item".to_string()],
                byte_start: 0,
                byte_end: code_text.len(),
            }],
            data: None,
        }));

        execute_chunking(&mut result, &config, None).unwrap();

        let chunks = result.chunks.expect("code chunks must be produced");
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].metadata.token_count,
            Some(code_text.split_whitespace().count()),
            "code chunk token_count must be populated from the registered tokenizer backend"
        );
    }
}
