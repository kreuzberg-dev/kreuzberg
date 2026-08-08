//! PDF page rendering using pdf_oxide.

use crate::Result;
use crate::core::diagnostics::{push_warning_deduped, warning};
use crate::error::XbergError;
use crate::types::ProcessingWarning;
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
use lopdf::{Document, ObjectId};
use std::cell::RefCell;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

/// `ProcessingWarning::source` used for glyphs the rasterizer could not paint.
///
/// See [`take_pdf_oxide_render_warnings`] for why this exists and where the
/// gap is upstream vs. xberg-side.
const PDF_RENDER_WARNING_SOURCE: &str = "pdf-render";

thread_local! {
    /// Buffer for `pdf_oxide`'s `log::warn!` records emitted while a render
    /// call made by this thread is in flight. `None` when no render call is
    /// currently capturing (the default, and the state between calls).
    static PDF_OXIDE_LOG_CAPTURE: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
    /// Deduped warnings drained from completed render calls on this thread,
    /// awaiting collection by [`take_pdf_oxide_render_warnings`].
    static PDF_OXIDE_PENDING_WARNINGS: RefCell<Vec<ProcessingWarning>> = const { RefCell::new(Vec::new()) };
}

static PDF_OXIDE_LOGGER_INIT: Once = Once::new();

/// Whether [`install_pdf_render_diagnostics`] actually won the global `log`
/// backend. Capture is skipped entirely while this is false, so the render
/// path costs nothing for the overwhelming majority of embedders who never
/// opt in.
static PDF_OXIDE_CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// A [`log::Log`] sink that captures `pdf_oxide`'s warning-level records into
/// the calling thread's [`PDF_OXIDE_LOG_CAPTURE`] buffer.
///
/// # Why this exists (#1364)
///
/// `pdf_oxide`'s rasterizer can silently drop a glyph — no font resolves for
/// the current run (`text_rasterizer.rs`: "No font found for '{}'..."),
/// parsing an embedded font fails (`page_renderer.rs`: "Failed to parse font
/// '{}'..."), the CJK predefined-CIDFont substitution face is unavailable, or
/// direct CID/CFF glyph-outline rendering errors mid-run. In every one of
/// those cases `pdf_oxide` still returns `Ok(RenderedImage { .. })` — the
/// page just has a gap where the glyph should be, with the text-space cursor
/// advanced as if it painted. `RenderedImage` carries no diagnostic field, so
/// none of this is visible to callers through the return value.
///
/// `pdf_oxide` *does* report every one of these cases through `log::warn!`,
/// but this crate never installed a [`log::Log`] backend, so — independent of
/// this fix — those records were going to the default no-op logger and were
/// dropped a second time. That is the exact upstream-plus-local gap #1364
/// describes: pdf_oxide's own diagnostic channel existed but nothing was
/// listening.
///
/// This is the xberg-side fix: install a capturing logger (once per process;
/// if the host application has already claimed the `log` facade for its own
/// logger, [`ensure_pdf_oxide_log_capture_installed`] leaves it alone and
/// this capture path silently yields nothing — no worse than today), and
/// during each render call collect `pdf_oxide`'s own target-prefixed warnings
/// into a `ProcessingWarning`. The actual *decision* about which glyph gets
/// dropped and why remains entirely inside `pdf_oxide`/`ttf-parser` — that
/// part is upstream and is not touched here.
struct PdfOxideWarningCapture;

impl log::Log for PdfOxideWarningCapture {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    /// Records outside `pdf_oxide` are dropped rather than re-emitted.
    ///
    /// Forwarding them to `tracing` is the obvious instinct — this sink owns the
    /// process's only `log` backend, so anything it drops is gone — but it does not
    /// work here and must not be reintroduced. The `tracing/log` feature is enabled
    /// in this build (pulled in through `tower`), which makes every `tracing` event
    /// also emit a `log` record. A forwarding sink therefore feeds itself: one
    /// `tracing::warn!` becomes a `log` record, which becomes a `tracing::warn!`,
    /// until the thread's stack is exhausted. That is not hypothetical — it aborted
    /// the #1364 regression test with `fatal runtime error: stack overflow`.
    ///
    /// Dropping them is acceptable precisely because installation is opt-in: before
    /// [`install_pdf_render_diagnostics`] is called there is no `log` backend at all
    /// and these records already go nowhere, so an application that opts in is
    /// choosing this trade knowingly rather than having it imposed.
    fn log(&self, record: &log::Record<'_>) {
        if !self.enabled(record.metadata()) || !record.target().starts_with("pdf_oxide") {
            return;
        }
        let message = record.args().to_string();
        PDF_OXIDE_LOG_CAPTURE.with(|cell| {
            if let Some(buffer) = cell.borrow_mut().as_mut() {
                buffer.push(message);
            }
        });
    }

    fn flush(&self) {}
}

/// Install [`PdfOxideWarningCapture`] as the process-wide `log` backend,
/// exactly once.
///
/// If another component already installed a `log::Log` implementation (an
/// application wiring `env_logger`, for instance), `log::set_boxed_logger`
/// fails and this is a no-op: we do not fight over ownership of the global
/// logger slot, and we do not touch `log::set_max_level` unless our install
/// won, so we never silently raise or lower a level someone else configured.
/// In that case `pdf_oxide`'s glyph-drop records go wherever that other
/// logger sends them instead of into [`take_pdf_oxide_render_warnings`].
/// **Opt-in.** Nothing calls this automatically, and that is deliberate: xberg
/// is a library, and `log` has exactly one global backend slot per process. A
/// library that claims it on its own behalf breaks its embedder — a host that
/// later calls `env_logger::init()` panics, and until this returns, every
/// `log` record in the process is routed here rather than wherever the host
/// intended. That decision belongs to the application, so it is exposed as a
/// call an application makes knowingly.
///
/// Returns `true` if this call (or an earlier one) installed the capture, and
/// `false` if some other component already owns the `log` backend — in which
/// case `pdf_oxide`'s glyph-drop records go to that logger and
/// [`take_pdf_oxide_render_warnings`] stays empty.
///
/// Without this call the #1364 warnings are not produced. The glyph drop
/// itself is decided inside `pdf_oxide`, which reports it only through
/// `log::warn!`; there is no return-value channel to read instead.
pub fn install_pdf_render_diagnostics() -> bool {
    PDF_OXIDE_LOGGER_INIT.call_once(|| {
        if log::set_boxed_logger(Box::new(PdfOxideWarningCapture)).is_ok() {
            // Only warnings are captured, so asking the `log` facade for anything
            // more verbose would cost every dependency a formatted record per call
            // for output this sink immediately discards.
            log::set_max_level(log::LevelFilter::Warn);
            PDF_OXIDE_CAPTURE_ACTIVE.store(true, Ordering::Release);
        }
    });
    PDF_OXIDE_CAPTURE_ACTIVE.load(Ordering::Acquire)
}

/// Turn one captured `pdf_oxide` log line into a `(page, message)`
/// [`ProcessingWarning`], naming the page so a multi-page document does not
/// read as "somewhere in this PDF, something happened".
fn glyph_drop_warning(page_index: usize, cause: &str) -> ProcessingWarning {
    warning(
        PDF_RENDER_WARNING_SOURCE,
        format!(
            "Page {} rendering could not paint one or more glyphs and continued anyway \
             (advance-only, so layout is preserved but the glyph ink is missing): {cause}",
            page_index + 1
        ),
    )
}

/// Render a page while capturing any `pdf_oxide` glyph-drop warnings it logs
/// during the call, deduping them into [`PDF_OXIDE_PENDING_WARNINGS`] for
/// later collection via [`take_pdf_oxide_render_warnings`].
///
/// Capture is opt-in: unless the application called
/// [`install_pdf_render_diagnostics`], this arms nothing and is exactly
/// equivalent to calling `render` directly, at no cost.
fn render_page_capturing_glyph_drops(
    page_index: usize,
    render: impl FnOnce() -> std::result::Result<pdf_oxide::rendering::RenderedImage, pdf_oxide::Error>,
) -> std::result::Result<pdf_oxide::rendering::RenderedImage, pdf_oxide::Error> {
    if !PDF_OXIDE_CAPTURE_ACTIVE.load(Ordering::Acquire) {
        return render();
    }
    PDF_OXIDE_LOG_CAPTURE.with(|cell| *cell.borrow_mut() = Some(Vec::new()));
    let result = render();
    let captured = PDF_OXIDE_LOG_CAPTURE.with(|cell| cell.borrow_mut().take().unwrap_or_default());
    if !captured.is_empty() {
        PDF_OXIDE_PENDING_WARNINGS.with(|pending| {
            let mut pending = pending.borrow_mut();
            for cause in &captured {
                push_warning_deduped(&mut pending, glyph_drop_warning(page_index, cause));
            }
        });
    }
    result
}

/// Drain the glyph-drop [`ProcessingWarning`]s accumulated on this thread by
/// render calls since the last call to this function.
///
/// Callers that render pages as part of extraction should call this after
/// their render pass and merge the result into
/// `InternalDocument::processing_warnings` (see the module-level convention
/// in `crate::core::diagnostics`) so a page with missing glyphs is never
/// returned to the user without a signal. Warnings are already deduped
/// per-thread across all pages rendered before this call.
///
/// `pub` (rather than `pub(crate)`) so both in-tree render-consumers and the
/// regression test for #1364 can observe capture without depending on any
/// one extractor's internal state.
///
/// As of #340, `crate::extractors::pdf::mod` drains this unconditionally right
/// after assembling a document's `processing_warnings`, so every PDF
/// extraction that renders at least one page picks up any captured
/// glyph-drop warnings for free. ~keep: that drain only ever observes
/// warnings from render calls that happened on the *same OS thread* before it
/// ran, because [`PDF_OXIDE_PENDING_WARNINGS`] is thread-local. OCR page
/// rendering runs inline on the extracting task's thread, so it is covered.
/// Layout-detection rasterization runs inside `tokio::task::spawn_blocking`,
/// which always executes on a different OS thread, so this function alone
/// would never see those warnings. As of #353,
/// `extractors::pdf::layout_runner::run_layout_for_pdf_pages_async` drains
/// this function itself from inside its `spawn_blocking` closure — the only
/// place that can observe the blocking-pool thread's thread-local buffer —
/// and threads the drained warnings back through its return value for the
/// caller in `extractors::pdf::mod` to merge, so layout-path glyph drops are
/// no longer silently lost.
pub fn take_pdf_oxide_render_warnings() -> Vec<ProcessingWarning> {
    PDF_OXIDE_PENDING_WARNINGS.with(|pending| std::mem::take(&mut *pending.borrow_mut()))
}

/// Reasonable max pixel dimension (on either axis) for a rendered page before we
/// force a lower DPI. This prevents Pixmap allocation failures or OOM for
/// extremely wide/tall technical diagrams, CAD exports, etc. while still
/// producing a usable raster for OCR/VLM (which are robust to moderate downscaling).
///
/// Chosen as 16384px because a 20000pt-wide page at the default 150 DPI produces
/// ~41667px on the long axis (20000 * 150 / 72), which triggers Pixmap creation
/// or rasterization failures inside pdf_oxide/tiny-skia for real vector-heavy
/// content. 16384 is high enough for normal documents (A3 landscape at 300dpi ~
/// 3500px) but catches the extreme cases reported in #1078. See the regression
/// test in this module for the exact repro input that previously failed.
const MAX_RENDER_DIMENSION_PX: f32 = 16384.0;

/// Compute a safe DPI for the given page MediaBox so that the rendered pixel
/// size stays within practical limits for the underlying rasterizer (tiny-skia
/// Pixmap + path/text rasterization in pdf_oxide).
///
/// Falls back to 72 DPI minimum. Returns the (possibly reduced) DPI to use.
fn choose_safe_dpi(w_pt: f32, h_pt: f32, base_dpi: u32) -> u32 {
    if w_pt <= 0.0 || h_pt <= 0.0 {
        return base_dpi.max(72);
    }
    let scale = base_dpi as f32 / 72.0;
    let w_px = w_pt * scale;
    let h_px = h_pt * scale;
    let max_dim = w_px.max(h_px);
    if max_dim <= MAX_RENDER_DIMENSION_PX {
        return base_dpi;
    }
    let factor = MAX_RENDER_DIMENSION_PX / max_dim;
    (base_dpi as f32 * factor).max(72.0) as u32
}

/// Fetch page MediaBox (in points) with a sane Letter fallback.
fn get_page_dimensions_pt(doc: &pdf_oxide::PdfDocument, page_index: usize) -> (f32, f32) {
    doc.get_page_media_box(page_index)
        .map(|(llx, lly, urx, ury)| ((urx - llx).abs(), (ury - lly).abs()))
        .unwrap_or((612.0, 792.0))
}

/// Maximum /Parent hops when resolving an inherited /Rotate attribute.
/// Bounds the walk so a malformed PDF with a parent cycle cannot loop forever.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
const MAX_ROTATE_INHERITANCE_DEPTH: usize = 32;

/// Resolve a page's effective /Rotate value, following /Parent inheritance
/// per the PDF spec (a page without its own /Rotate inherits from its Pages
/// ancestors). Returns `None` when no ancestor defines it.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
fn resolve_inherited_rotation(doc: &Document, page_id: ObjectId) -> Option<i64> {
    let mut dict = doc.get_object(page_id).ok()?.as_dict().ok()?;
    for _ in 0..MAX_ROTATE_INHERITANCE_DEPTH {
        if let Ok(rotate_obj) = dict.get(b"Rotate") {
            return rotate_obj.as_i64().ok();
        }
        let parent_id = dict.get(b"Parent").ok()?.as_reference().ok()?;
        dict = doc.get_object(parent_id).ok()?.as_dict().ok()?;
    }
    None
}

/// Read per-page /Rotate values for a whole document, normalized to
/// 0/90/180/270 (negative multiples of 90 are folded via `rem_euclid`).
///
/// Parses the PDF once with lopdf; a parse failure or missing attribute
/// yields 0 (no rotation) for the affected pages. lopdf's `get_pages()`
/// map is keyed by 1-based page number, which is the authoritative page
/// order (object IDs are not ordered by page).
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
pub(crate) fn get_page_rotations(pdf_bytes: &[u8], page_count: usize) -> Vec<u32> {
    let mut rotations = vec![0u32; page_count];
    let Ok(doc) = Document::load_mem(pdf_bytes) else {
        return rotations;
    };
    for (page_number, page_id) in doc.get_pages() {
        let index = (page_number as usize).saturating_sub(1);
        if index >= page_count {
            continue;
        }
        if let Some(rotate_int) = resolve_inherited_rotation(&doc, page_id) {
            rotations[index] = rotate_int.rem_euclid(360) as u32;
        }
    }
    rotations
}

/// Rotate a decoded page image per the page's normalized /Rotate value.
/// No-op for 0 or non-quarter-turn values.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
pub(crate) fn rotate_dynamic_image(img: image::DynamicImage, rotation_degrees: u32) -> image::DynamicImage {
    match rotation_degrees % 360 {
        90 => img.rotate90(),
        180 => img.rotate180(),
        270 => img.rotate270(),
        _ => img,
    }
}

/// Return the correction needed to make a page raster upright after
/// `pdf_oxide` has applied the PDF page's `/Rotate` value while rendering.
/// ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
pub(crate) fn ocr_page_correction_degrees(rotation_degrees: u32) -> u32 {
    (360 - rotation_degrees % 360) % 360
}

/// Rotate PNG-encoded page bytes per the page's /Rotate value.
///
/// Fast path: rotation 0 returns the input unchanged (no decode). Rotated
/// pages pay one decode + re-encode, which only happens for documents that
/// actually carry /Rotate. Returns the (possibly new) PNG bytes with the
/// post-rotation width and height.
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
pub(crate) fn rotate_png_page_if_needed(
    png_data: Vec<u8>,
    width: u32,
    height: u32,
    rotation_degrees: u32,
) -> Result<(Vec<u8>, u32, u32)> {
    if rotation_degrees.is_multiple_of(360) {
        return Ok((png_data, width, height));
    }
    let img = image::load_from_memory(&png_data).map_err(|e| XbergError::Parsing {
        message: format!("failed to decode rendered page for rotation correction: {e}"),
        source: None,
    })?;
    let rotated = rotate_dynamic_image(img, rotation_degrees);
    let (w, h) = (rotated.width(), rotated.height());
    let mut buf = Vec::new();
    rotated
        .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
        .map_err(|e| XbergError::Parsing {
            message: format!("failed to re-encode rotated page: {e}"),
            source: None,
        })?;
    Ok((buf, w, h))
}

/// Normalize a `pdf_oxide` page raster for OCR.
///
/// `pdf_oxide` already applies `/Rotate` to the rendered page. OCR needs the
/// inverse transform exactly once so text is upright before layout and OCR
/// inference consume the shared raster. ~keep
#[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
pub(crate) fn normalize_rendered_page_for_ocr(
    png_data: Vec<u8>,
    width: u32,
    height: u32,
    rotation_degrees: u32,
) -> Result<(Vec<u8>, u32, u32)> {
    rotate_png_page_if_needed(png_data, width, height, ocr_page_correction_degrees(rotation_degrees))
}

/// Contain a panic raised while rasterizing one page, turning it into an
/// ordinary `pdf_oxide::Error` for that page.
///
/// Rasterization runs third-party code over attacker-controlled geometry, and a
/// malformed page can violate an invariant the rasterizer only asserts: on a PDF
/// whose content streams fail to inflate (`FlateDecode` recovery exhausted),
/// tiny-skia 0.12.0 unwraps an `AlphaRun` in `break_run` (`alpha_runs.rs:170`)
/// that is `None`, and panics. There is no newer tiny-skia to upgrade to —
/// 0.12.0 is the current release.
///
/// Because render calls run synchronously on a Tokio worker, that panic unwinds
/// through the async boundary and fails the entire request with a 500, so a
/// single bad page costs the caller every other page's text as well. Containing
/// it here is the same treatment the text path gives the total-order sort panic
/// (#1198): the page becomes an `Err`, callers fall back to their existing
/// render-failure handling, and the rest of the document still extracts.
fn guard_render_panic(
    page_index: usize,
    render: impl FnOnce() -> std::result::Result<pdf_oxide::rendering::RenderedImage, pdf_oxide::Error>,
) -> std::result::Result<pdf_oxide::rendering::RenderedImage, pdf_oxide::Error> {
    super::oxide::guard_oxide_panic(render, |message| {
        pdf_oxide::Error::InvalidPdf(format!(
            "page {} could not be rasterized: the rasterizer panicked and was contained ({message})",
            page_index + 1
        ))
    })
}

/// Render a page using safeguards for extreme dimensions (wide vector diagrams,
/// CAD sheets, etc.). This is the root-cause fix for render failures on such
/// inputs during force_ocr / VLM / layout paths.
///
/// Uses the opened document (so callers that batch multiple pages only parse once).
pub(crate) fn render_page_with_safeguards(
    doc: &pdf_oxide::PdfDocument,
    page_index: usize,
    base_dpi: u32,
) -> std::result::Result<pdf_oxide::rendering::RenderedImage, pdf_oxide::Error> {
    let (w_pt, h_pt) = get_page_dimensions_pt(doc, page_index);
    let safe_dpi = choose_safe_dpi(w_pt, h_pt, base_dpi);
    if safe_dpi != base_dpi {
        tracing::warn!(
            page = page_index + 1,
            original_dpi = base_dpi,
            effective_dpi = safe_dpi,
            width_pt = w_pt,
            height_pt = h_pt,
            "reducing render DPI for page due to extreme dimensions (wide vector-heavy PDF or similar)"
        );
    }
    let options = pdf_oxide::rendering::RenderOptions::with_dpi(safe_dpi);
    // The panic guard sits inside the capture wrapper, not around it, so a
    // panicking page still lets the wrapper take its thread-local buffer back
    // instead of leaving it armed on a pooled thread.
    render_page_capturing_glyph_drops(page_index, || {
        guard_render_panic(page_index, || {
            pdf_oxide::rendering::render_page(doc, page_index, &options)
        })
    })
}

/// Open (and optionally authenticate) a PDF document from raw bytes.
///
/// Parsing the cross-reference table and trailer is the expensive part of
/// working with a PDF; rendering a page only reads the already-parsed
/// structures. Callers that need several pages should open the document once
/// with this helper and reuse the returned handle across
/// [`render_open_pdf_page_to_png`] calls rather than re-opening per page.
///
/// # Errors
///
/// Returns `XbergError::Parsing` if the PDF cannot be opened or authenticated.
pub(crate) fn open_pdf_document(pdf_bytes: &[u8], password: Option<&str>) -> Result<pdf_oxide::PdfDocument> {
    let doc = pdf_oxide::PdfDocument::from_bytes(pdf_bytes.to_vec()).map_err(|e| XbergError::Parsing {
        message: format!("Failed to open PDF: {e}"),
        source: None,
    })?;

    if let Some(pwd) = password {
        doc.authenticate(pwd.as_bytes()).map_err(|e| XbergError::Parsing {
            message: format!("Failed to authenticate PDF: {e}"),
            source: None,
        })?;
    }

    Ok(doc)
}

/// Read the page count from an already-open document.
///
/// # Errors
///
/// Returns `XbergError::Parsing` if the page count cannot be read.
pub(crate) fn document_page_count(doc: &pdf_oxide::PdfDocument) -> Result<usize> {
    doc.page_count().map_err(|e| XbergError::Parsing {
        message: format!("Failed to read page count: {e}"),
        source: None,
    })
}

/// Render one page of an already-open document to PNG bytes via the
/// extreme-dimension DPI safeguard.
///
/// This is the per-page primitive shared by [`render_pdf_page_to_png`] (which
/// opens the document, then delegates) and batch callers that open once and
/// render every page from a single parsed handle. `page_index` is assumed to be
/// in range; out-of-range indices surface as the underlying rasterizer error.
///
/// # Errors
///
/// Returns `XbergError::Parsing` if the page cannot be rendered.
pub(crate) fn render_open_pdf_page_to_png(
    doc: &pdf_oxide::PdfDocument,
    page_index: usize,
    dpi: Option<i32>,
) -> Result<Vec<u8>> {
    let render_dpi = dpi.unwrap_or(150).max(1) as u32;
    let rendered = render_page_with_safeguards(doc, page_index, render_dpi).map_err(|e| XbergError::Parsing {
        message: format!("Failed to render page {page_index}: {e}"),
        source: None,
    })?;

    Ok(rendered.data)
}

/// Render a single PDF page to PNG bytes.
///
/// Returns raw PNG-encoded bytes for the specified page at the given DPI.
/// Uses pdf_oxide with tiny-skia for pure-Rust rendering.
///
/// For pages with extreme dimensions (very wide vector diagrams, etc.) the
/// effective DPI may be automatically reduced to avoid rasterizer failure.
/// A warning is logged when this happens.
///
/// # Arguments
///
/// * `pdf_bytes` - Raw PDF file bytes
/// * `page_index` - Zero-based page index
/// * `dpi` - Resolution in dots per inch (default: 150)
/// * `password` - Optional password for encrypted PDFs
///
/// # Errors
///
/// Returns `XbergError::Parsing` if the PDF cannot be opened, authenticated,
/// or rendered, or if `page_index` is out of range.
pub fn render_pdf_page_to_png(
    pdf_bytes: &[u8],
    page_index: usize,
    dpi: Option<i32>,
    password: Option<&str>,
) -> Result<Vec<u8>> {
    let doc = open_pdf_document(pdf_bytes, password)?;

    let page_count = document_page_count(&doc)?;
    if page_index >= page_count {
        return Err(XbergError::Parsing {
            message: format!("Page index {page_index} out of range (document has {page_count} pages)"),
            source: None,
        });
    }

    render_open_pdf_page_to_png(&doc, page_index, dpi)
}

/// Count the pages in a PDF without rendering any of them.
///
/// Opens the document and returns its page count from the PDF structure. No page
/// is rasterized, so this is cheap relative to `render_pdf_page_to_png` — use it
/// when you only need the count (e.g. to drive a render loop over the pages).
///
/// # Arguments
///
/// * `pdf_bytes` - Raw PDF file bytes
/// * `password` - Optional password for encrypted PDFs
///
/// # Errors
///
/// Returns `XbergError::Parsing` if the PDF cannot be opened, authenticated,
/// or its page count read.
pub fn pdf_page_count(pdf_bytes: &[u8], password: Option<&str>) -> Result<usize> {
    let doc = pdf_oxide::PdfDocument::from_bytes(pdf_bytes.to_vec()).map_err(|e| XbergError::Parsing {
        message: format!("Failed to open PDF: {e}"),
        source: None,
    })?;

    if let Some(pwd) = password {
        doc.authenticate(pwd.as_bytes()).map_err(|e| XbergError::Parsing {
            message: format!("Failed to authenticate PDF: {e}"),
            source: None,
        })?;
    }

    doc.page_count().map_err(|e| XbergError::Parsing {
        message: format!("Failed to read page count: {e}"),
        source: None,
    })
}

/// Build a minimal valid single-page PDF with the given MediaBox (in points).
/// Used to test the wide-page / extreme-dimension safeguard in the renderer.
/// Note: the generated PDF has no content stream or /Resources. It is sufficient
/// to exercise the MediaBox-based DPI guard, but real-world wide vector diagrams
/// with complex paths may exercise additional failure modes in the rasterizer.
/// This is a known limitation of the in-memory test; a real repro PDF from #1078
/// was used during manual verification.
#[cfg(all(test, feature = "pdf"))]
pub(crate) fn build_minimal_pdf_with_mediabox(w: f32, h: f32) -> Vec<u8> {
    let mut buf = Vec::<u8>::new();
    buf.extend_from_slice(b"%PDF-1.4\n");

    let obj1_offset = buf.len();
    buf.extend_from_slice(b"1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");

    let obj2_offset = buf.len();
    buf.extend_from_slice(b"2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");

    let obj3_offset = buf.len();
    let mb = format!("[0 0 {} {}]", w, h);
    buf.extend_from_slice(format!("3 0 obj\n<</Type /Page /MediaBox {} /Parent 2 0 R>>\nendobj\n", mb).as_bytes());

    let xref_offset = buf.len();

    buf.extend_from_slice(b"xref\n");
    buf.extend_from_slice(b"0 4\n");
    buf.extend_from_slice(b"0000000000 65535 f \n");
    buf.extend_from_slice(format!("{:010} 00000 n \n", obj1_offset).as_bytes());
    buf.extend_from_slice(format!("{:010} 00000 n \n", obj2_offset).as_bytes());
    buf.extend_from_slice(format!("{:010} 00000 n \n", obj3_offset).as_bytes());

    buf.extend_from_slice(b"trailer\n<</Size 4 /Root 1 0 R>>\n");
    buf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

    buf
}

/// Build a single-page PDF embedding a synthetic Type 1C (CFF) font whose
/// dot-bearing glyphs carry the deprecated `dotsection` operator, mirroring
/// Adobe's Type 1 to Type 2 converter output that surfaced the bug. The font
/// is generated with fontTools (no third-party font data); stock ttf-parser
/// 0.25.1 drops all seven dotsection glyphs from it while the controls keep
/// their outlines, so the patched parser (which carries the fix) is what makes
/// this render. Used by the dotsection regression test below.
///
/// Layout (48pt glyphs, one per 72pt-wide cell starting at x=72):
///   row 1, baseline y=650: i j . : ; ! ?   (all carry dotsection)
///   row 2, baseline y=450: l n ,           (controls, no dotsection)
///
/// Each glyph is drawn with its own `Td`, so ink positions are independent of
/// advance widths. Poppler renders every cell of this exact layout.
#[cfg(all(test, feature = "pdf"))]
pub(crate) fn build_dotsection_cff_pdf() -> Vec<u8> {
    const CFF: &[u8] = include_bytes!("testdata/dotsection_test_font.cff");
    const DIFFERENCES: &str = "[ 33 /exclam 44 /comma 46 /period 58 /colon 59 /semicolon \
                               63 /question 105 /i 106 /j 108 /l 110 /n ]";

    let mut content = String::new();
    for (codes, y) in [(DOTSECTION_ROW, 650u32), (CONTROL_ROW, 450u32)] {
        for (k, code) in codes.iter().enumerate() {
            let x = 72 + k * 72;
            content.push_str(&format!("BT /F1 48 Tf {x} {y} Td <{code:02X}> Tj ET\n"));
        }
    }

    let widths = (33..=110).map(|_| "500").collect::<Vec<_>>().join(" ");
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
           /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .to_vec(),
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /XbergDotsectionTest \
             /FirstChar 33 /LastChar 110 /Widths [ {widths} ] \
             /FontDescriptor 6 0 R /Encoding 7 0 R >>"
        )
        .into_bytes(),
        {
            let mut o = format!("<< /Length {} >>\nstream\n", content.len()).into_bytes();
            o.extend_from_slice(content.as_bytes());
            o.extend_from_slice(b"endstream");
            o
        },
        b"<< /Type /FontDescriptor /FontName /XbergDotsectionTest /Flags 32 \
           /FontBBox [-200 -250 1000 1000] /ItalicAngle 0 /Ascent 800 \
           /Descent -200 /CapHeight 700 /StemV 80 /FontFile3 8 0 R >>"
            .to_vec(),
        format!("<< /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences {DIFFERENCES} >>").into_bytes(),
        {
            let mut o = format!("<< /Subtype /Type1C /Length {} >>\nstream\n", CFF.len()).into_bytes();
            o.extend_from_slice(CFF);
            o.extend_from_slice(b"\nendstream");
            o
        },
    ];

    let mut buf = Vec::<u8>::new();
    buf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (i, body) in objects.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = buf.len();
    buf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    buf.extend_from_slice(b"0000000000 65535 f \n");
    for off in offsets {
        buf.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\n", objects.len() + 1).as_bytes());
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    buf
}

/// Char codes of the dotsection-carrying glyphs in [`build_dotsection_cff_pdf`]:
/// i, j, period, colon, semicolon, exclam, question.
#[cfg(all(test, feature = "pdf"))]
pub(crate) const DOTSECTION_ROW: &[u8] = &[105, 106, 46, 58, 59, 33, 63];

/// Char codes of the control glyphs (no dotsection): l, n, comma.
#[cfg(all(test, feature = "pdf"))]
pub(crate) const CONTROL_ROW: &[u8] = &[108, 110, 44];

#[cfg(all(test, feature = "pdf"))]
mod tests {
    use super::*;

    #[test]
    fn test_choose_safe_dpi_normal_page_unchanged() {
        let dpi = choose_safe_dpi(612.0, 792.0, 150);
        assert_eq!(dpi, 150);
    }

    #[test]
    fn test_choose_safe_dpi_extreme_wide_reduced() {
        let dpi = choose_safe_dpi(20000.0, 200.0, 150);
        assert_eq!(dpi, 72);
    }

    #[test]
    fn test_render_pdf_page_to_png_very_wide_does_not_panic_or_hard_fail() {
        let wide_pdf = build_minimal_pdf_with_mediabox(20000.0, 300.0);
        let res = render_pdf_page_to_png(&wide_pdf, 0, None, None);
        assert!(
            res.is_ok(),
            "wide page render should succeed thanks to safeguard, got: {:?}",
            res.err()
        );
    }

    // A rasterizer panic used to unwind through the Tokio worker and fail the
    // whole extraction with a 500, so the other pages' text was lost with it.
    #[test]
    fn test_guard_render_panic_contains_panic_as_page_error() {
        // Matched rather than `expect_err`, which would require RenderedImage: Debug.
        let message = match guard_render_panic(3, || panic!("simulated tiny-skia unwrap")) {
            Ok(_) => panic!("panic must not escape the guard"),
            Err(error) => error.to_string(),
        };
        assert!(
            message.contains("page 4"),
            "error should name the 1-based page: {message}"
        );
        assert!(
            message.contains("simulated tiny-skia unwrap"),
            "error should carry the panic message: {message}"
        );
    }

    #[test]
    fn test_pdf_page_count_single_page() {
        let pdf = build_minimal_pdf_with_mediabox(612.0, 792.0);
        let count = pdf_page_count(&pdf, None).expect("page count should succeed for a valid PDF");
        assert_eq!(count, 1, "minimal single-page PDF must report 1 page");
    }

    #[test]
    fn test_pdf_page_count_invalid_pdf_errors() {
        let err = pdf_page_count(b"not a pdf", None).expect_err("invalid PDF bytes must error");
        assert!(
            matches!(err, XbergError::Parsing { .. }),
            "expected a Parsing error, got: {err:?}"
        );
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_rotate_dynamic_image_0_degrees_is_noop() {
        let img = image::DynamicImage::new_rgb8(100, 150);
        let rotated = rotate_dynamic_image(img, 0);
        assert_eq!((rotated.width(), rotated.height()), (100, 150));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_rotate_dynamic_image_90_degrees_swaps_dimensions() {
        let img = image::DynamicImage::new_rgb8(100, 150);
        let rotated = rotate_dynamic_image(img, 90);
        assert_eq!((rotated.width(), rotated.height()), (150, 100));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_rotate_dynamic_image_180_degrees_keeps_dimensions() {
        let img = image::DynamicImage::new_rgb8(100, 150);
        let rotated = rotate_dynamic_image(img, 180);
        assert_eq!((rotated.width(), rotated.height()), (100, 150));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_rotate_dynamic_image_270_degrees_swaps_dimensions() {
        let img = image::DynamicImage::new_rgb8(100, 150);
        let rotated = rotate_dynamic_image(img, 270);
        assert_eq!((rotated.width(), rotated.height()), (150, 100));
    }

    /// Count pixels darker than mid-gray inside one glyph cell of the
    /// dotsection fixture. Cells are 72pt wide starting at x=72; the y band
    /// covers ascender through descender around the row's baseline.
    fn dark_pixels_in_cell(img: &image::GrayImage, cell: usize, baseline_pt: f32) -> u32 {
        const SCALE: f32 = 150.0 / 72.0;
        const PAGE_H_PT: f32 = 792.0;
        let x0 = (((72 + cell * 72) as f32 - 4.0) * SCALE).max(0.0) as u32;
        let x1 = ((((72 + cell * 72) + 56) as f32) * SCALE).min(img.width() as f32) as u32;
        let y0 = ((PAGE_H_PT - (baseline_pt + 40.0)) * SCALE).max(0.0) as u32;
        let y1 = ((PAGE_H_PT - (baseline_pt - 14.0)) * SCALE).min(img.height() as f32) as u32;
        let mut dark = 0u32;
        for y in y0..y1 {
            for x in x0..x1 {
                if img.get_pixel(x, y).0[0] < 128 {
                    dark += 1;
                }
            }
        }
        dark
    }

    /// Regression test for CFF fonts whose charstrings carry the deprecated
    /// `dotsection` operator (12 0). ttf-parser 0.25.1 aborted the whole
    /// charstring with `UnsupportedOperator`, so pdf_oxide painted nothing for
    /// i, j, period, colon, semicolon, exclam and question while still
    /// advancing the cursor: OCR received page images with those letters
    /// silently missing. Exercises the full render path against the parser the
    /// workspace `[patch.crates-io]` routes to — `xberg-ttf-parser`, which
    /// carries the fix (upstream #228).
    #[test]
    fn test_render_paints_cff_glyphs_that_use_dotsection() {
        let names_row1 = ["i", "j", "period", "colon", "semicolon", "exclam", "question"];
        let names_row2 = ["l", "n", "comma"];

        let pdf = build_dotsection_cff_pdf();
        let png = render_pdf_page_to_png(&pdf, 0, Some(150), None).expect("fixture page must render");
        let img = image::load_from_memory(&png)
            .expect("rendered PNG must decode")
            .to_luma8();

        for (names, codes, baseline) in [
            (&names_row1[..], DOTSECTION_ROW, 650.0),
            (&names_row2[..], CONTROL_ROW, 450.0),
        ] {
            for (cell, name) in names.iter().enumerate() {
                let dark = dark_pixels_in_cell(&img, cell, baseline);
                assert!(
                    dark >= 3,
                    "glyph '{name}' (char code {}) rendered no ink: {dark} dark pixels in its cell; \
                     dotsection charstrings must produce outlines",
                    codes[cell],
                );
            }
        }
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
    #[test]
    fn ocr_page_correction_inverts_pdf_rotation() {
        assert_eq!(ocr_page_correction_degrees(0), 0);
        assert_eq!(ocr_page_correction_degrees(90), 270);
        assert_eq!(ocr_page_correction_degrees(180), 180);
        assert_eq!(ocr_page_correction_degrees(270), 90);
        assert_eq!(ocr_page_correction_degrees(360), 0);
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
    #[test]
    fn rendered_page_ocr_normalization_applies_inverse_quarter_turns() {
        let marker = image::Rgb([17, 31, 47]);
        let mut source = image::RgbImage::new(3, 2);
        source.put_pixel(0, 0, marker);
        let mut encoded = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut encoded, image::ImageFormat::Png)
            .expect("test image should encode");
        let encoded = encoded.into_inner();

        let cases = [
            (0, (3, 2), (0, 0)),
            (90, (2, 3), (0, 2)),
            (180, (3, 2), (2, 1)),
            (270, (2, 3), (1, 0)),
        ];
        for (pdf_rotation, expected_dimensions, marker_position) in cases {
            let (normalized, width, height) = normalize_rendered_page_for_ocr(encoded.clone(), 3, 2, pdf_rotation)
                .expect("in-memory OCR normalization should succeed");
            let image = image::load_from_memory(&normalized)
                .expect("normalized PNG should decode")
                .to_rgb8();

            assert_eq!((width, height), expected_dimensions, "PDF rotation {pdf_rotation}");
            assert_eq!(image.dimensions(), expected_dimensions, "PDF rotation {pdf_rotation}");
            assert_eq!(
                *image.get_pixel(marker_position.0, marker_position.1),
                marker,
                "PDF rotation {pdf_rotation}"
            );
        }
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
    #[test]
    fn test_get_page_rotations_no_rotate_attribute_yields_zeroes() {
        let pdf = build_minimal_pdf_with_mediabox(612.0, 792.0);
        assert_eq!(get_page_rotations(&pdf, 1), vec![0]);
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline", feature = "layout-detection"))]
    #[test]
    fn test_get_page_rotations_unparsable_bytes_yield_zeroes() {
        assert_eq!(get_page_rotations(b"not a pdf", 3), vec![0, 0, 0]);
    }
}
