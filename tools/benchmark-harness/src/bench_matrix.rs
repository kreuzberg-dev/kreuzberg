//! Pinned benchmark artifact release contract: cohort manifests, the matrix of expected
//! framework x output-format x execution-mode cells, and their derived aggregate keys.
//!
//! Ported from `scripts/ci/benchmarks/validate-benchmark-artifacts.py`. The digests and matrix
//! shape below are pinned release-contract values copied verbatim from that script; they must
//! only change alongside a deliberate cohort/matrix revision, never as an incidental refactor.

use crate::aggregate::make_aggregate_key;
use crate::config::BenchmarkMode;
use crate::types::OutputFormat;
use crate::{Error, Result};

/// Execution mode of one matrix cell, and the three string encodings the release contract
/// expects it to appear as (artifact-directory slug, aggregate-key slug, provenance enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    /// Sequential single-document execution.
    SingleFile,
    /// Concurrent/native-batch execution.
    Batch,
}

impl ExecutionMode {
    /// Slug used in artifact directory names, e.g. `benchmarks-docling-markdown-single-file-<cohort>`.
    pub fn artifact_slug(self) -> &'static str {
        match self {
            ExecutionMode::SingleFile => "single-file",
            ExecutionMode::Batch => "batch",
        }
    }

    /// Slug used in `by_framework_mode` aggregate keys and `execution_mode` fixture rows.
    pub fn aggregate_slug(self) -> &'static str {
        match self {
            ExecutionMode::SingleFile => "single",
            ExecutionMode::Batch => "batch",
        }
    }

    /// The `TimingProvenance::mode` value a run in this execution mode must record.
    pub fn benchmark_mode(self) -> BenchmarkMode {
        match self {
            ExecutionMode::SingleFile => BenchmarkMode::SingleFile,
            ExecutionMode::Batch => BenchmarkMode::Batch,
        }
    }
}

/// One expected artifact cell in a cohort's release contract.
#[derive(Debug, Clone)]
pub struct MatrixEntry {
    /// Artifact directory name, without the trailing `-<run-id>` suffix.
    pub artifact: String,
    /// Framework name this cell was produced by.
    pub framework: String,
    /// Output format this cell was produced with.
    pub output_format: OutputFormat,
    /// Execution mode this cell was produced with.
    pub mode: ExecutionMode,
    /// When true, this cell is best-effort: its absence must not fail validation and it is
    /// excluded from the required release contract (e.g. MinerU, whose offline model-config
    /// fetch can hang). A present, well-formed optional artifact still flows into the
    /// aggregate; the validator simply does not gate publication on it.
    pub optional: bool,
}

impl MatrixEntry {
    /// Derive this cell's `by_framework_mode` aggregate key by delegating to the same key
    /// builder the `consolidate` command uses, so the contract can never drift from it.
    pub fn aggregate_key(&self) -> String {
        make_aggregate_key(&self.framework, self.output_format, self.mode.aggregate_slug())
    }

    /// Mark this cell best-effort (non-contractual). See [`MatrixEntry::optional`].
    pub fn into_optional(mut self) -> Self {
        self.optional = true;
        self
    }
}

/// The exact, pinned artifact contract for one benchmark cohort.
#[derive(Debug, Clone)]
pub struct CohortContract {
    /// Cohort manifest `name` field (e.g. `native-pdf-fast-b8-v1`).
    pub manifest_name: &'static str,
    /// Pinned BLAKE3 digest of the cohort manifest file's exact bytes.
    pub manifest_blake3: &'static str,
    /// Required native-batch document count.
    pub batch_size: usize,
    /// Fixture descriptor paths, in the exact pinned order.
    pub fixtures: &'static [&'static str],
    /// Expected document basename stems, in fixture order.
    pub document_stems: &'static [&'static str],
    /// Expected document file extensions (lowercase, the `by_file_type` bucket keys), in fixture
    /// order. Lets the aggregate validator assert each file-type bucket's sample count matches how
    /// many of the cohort's fixtures are of that extension, not merely that the total sums right.
    pub document_extensions: &'static [&'static str],
    /// Every expected framework x output-format x execution-mode cell.
    pub matrix: Vec<MatrixEntry>,
}

/// Which cohort's release contract is being validated. PDF cohorts (`Native`, `Ocr`) and the
/// image cohort exercise the rendered-page layout pipeline, so their xberg cells run both the
/// `baseline` and `layout` variants; the remaining per-format-family cohorts have no rendered
/// page, so xberg runs `baseline` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cohort {
    /// Native (non-OCR) fast PDF cohort.
    Native,
    /// OCR fast PDF cohort.
    Ocr,
    /// Word-processing / presentation / spreadsheet family (docx, doc, pptx, ppt, xlsx, odt, rtf).
    Office,
    /// Markup / typesetting family (html, md, latex, typst, rst, org, docbook).
    Markup,
    /// E-book family (epub, fb2).
    Ebook,
    /// E-mail family (eml, msg).
    Email,
    /// Structured-data family (csv, tsv, json, yaml).
    Data,
    /// OCR document-image family (png, jpeg, tiff).
    Images,
}

impl Cohort {
    /// Every cohort, in canonical order.
    pub const ALL: [Cohort; 8] = [
        Cohort::Native,
        Cohort::Ocr,
        Cohort::Office,
        Cohort::Markup,
        Cohort::Ebook,
        Cohort::Email,
        Cohort::Data,
        Cohort::Images,
    ];

    /// The `--cohort` CLI value for this cohort.
    pub fn as_str(self) -> &'static str {
        match self {
            Cohort::Native => "native",
            Cohort::Ocr => "ocr",
            Cohort::Office => "office",
            Cohort::Markup => "markup",
            Cohort::Ebook => "ebook",
            Cohort::Email => "email",
            Cohort::Data => "data",
            Cohort::Images => "images",
        }
    }

    /// Parse a `--cohort` CLI value.
    pub fn parse(value: &str) -> Result<Self> {
        Cohort::ALL
            .into_iter()
            .find(|cohort| cohort.as_str() == value)
            .ok_or_else(|| {
                let names = Cohort::ALL.map(Cohort::as_str).join(", ");
                Error::Config(format!("unknown cohort '{value}': expected one of: {names}"))
            })
    }

    /// Whether every result in this cohort's artifacts is expected to report OCR as *used*.
    ///
    /// Only the scanned-PDF cohort qualifies. The image cohort still runs with OCR enabled
    /// (`ocr_enabled` in its manifest), but xberg treats OCR as the intrinsic text path for a
    /// bare image rather than a fallback, so its results report `OcrStatus::NotUsed` — the same
    /// bucket a native cohort lands in. Requiring `Used` here would make every image result fail
    /// validation.
    pub fn expects_ocr(self) -> bool {
        matches!(self, Cohort::Ocr)
    }

    /// Whether this cohort's xberg cells include the rendered-page `layout` pipeline variant in
    /// addition to `baseline`. Only formats with a rendered page (PDF, images) benefit; for the
    /// other families a `layout` run would merely duplicate `baseline`.
    pub fn includes_layout(self) -> bool {
        matches!(self, Cohort::Native | Cohort::Ocr | Cohort::Images)
    }

    /// Whether this cohort's xberg cells include the PaddleOCR-engine variants (`baseline-paddle`,
    /// and `layout-paddle` when layout is also included) alongside the default Tesseract-backed
    /// `baseline`/`layout` runs. Only the OCR cohorts (scanned PDF, images) force OCR, so only they
    /// benefit from a second OCR engine; the native/office/markup/... families never run OCR.
    pub fn includes_paddle_ocr(self) -> bool {
        matches!(self, Cohort::Ocr | Cohort::Images)
    }

    /// Whether this release cohort includes the normal ONNX Runtime Sceptre variants.
    pub fn includes_sceptre_ort(self) -> bool {
        matches!(self, Cohort::Ocr | Cohort::Images)
    }

    /// Build this cohort's pinned release contract.
    pub fn contract(self) -> CohortContract {
        match self {
            Cohort::Native => native_contract(),
            Cohort::Ocr => ocr_contract(),
            Cohort::Office => office_contract(),
            Cohort::Markup => markup_contract(),
            Cohort::Ebook => ebook_contract(),
            Cohort::Email => email_contract(),
            Cohort::Data => data_contract(),
            Cohort::Images => images_contract(),
        }
    }
}

const NATIVE_COHORT: &str = "native-pdf-fast-b8";
const OCR_COHORT: &str = "ocr-pdf-fast-b4";

const NATIVE_MANIFEST_NAME: &str = "native-pdf-fast-b8-v1";
const NATIVE_MANIFEST_BLAKE3: &str = "3c32b27d07eb604a144661701daeada440c78dfd2d7344d29b7898c4114b2d81";
const NATIVE_BATCH_SIZE: usize = 8;
const NATIVE_FIXTURES: &[&str] = &[
    "pdf_tiny_memo.json",
    "pdf_tables.json",
    "pdf_embedded.json",
    "pdf_google_docs.json",
    "pdf/681693.json",
    "pdf/ft_ACN_2009_page_102_t0.json",
    "pdf/pb_fqr-retail-blackrock-global-allocation-fund-inc_page4.json",
    "pdf/pb_sample_page_16_page1.json",
];
const NATIVE_DOCUMENT_STEMS: &[&str] = &[
    "fake_memo",
    "tiny",
    "embedded_images_tables",
    "google_doc_document",
    "681693",
    "ft_ACN_2009_page_102_t0",
    "pb_fqr-retail-blackrock-global-allocation-fund-inc_page4",
    "pb_sample_page_16_page1",
];
const NATIVE_DOCUMENT_EXTENSIONS: &[&str] = &["pdf", "pdf", "pdf", "pdf", "pdf", "pdf", "pdf", "pdf"];

const OCR_MANIFEST_NAME: &str = "ocr-pdf-fast-b4-v1";
const OCR_MANIFEST_BLAKE3: &str = "36ee44b06085516a7f78fd40af17b31ce7833ad2a164efc38b044aa8ecea0bc8";
const OCR_BATCH_SIZE: usize = 4;
const OCR_FIXTURES: &[&str] = &[
    "pdf_non_searchable.json",
    "pdf_ocr_test.json",
    "pdf_scanned_ocr.json",
    "pdf_image_only_german.json",
];
const OCR_DOCUMENT_STEMS: &[&str] = &["non_searchable", "ocr_test", "scanned", "image_only_german_pdf"];
const OCR_DOCUMENT_EXTENSIONS: &[&str] = &["pdf", "pdf", "pdf", "pdf"];

const OFFICE_COHORT: &str = "native-office-fast";
const OFFICE_MANIFEST_NAME: &str = "native-office-fast-v1";
const OFFICE_MANIFEST_BLAKE3: &str = "f732f8eb582d58b56492b047cfade8d2298861fb3313675595d8162037e6960e";
const OFFICE_BATCH_SIZE: usize = 8;
const OFFICE_FIXTURES: &[&str] = &[
    "docx_simple.json",
    "docx/docx_tables.json",
    "doc/unit_test_lists.json",
    "pptx/powerpoint_sample.json",
    "ppt_presentation.json",
    "xlsx/excel_multi_sheet.json",
    "odt/headers.json",
    "rtf/formatting.json",
    "docx/quarterly_operations_report.json",
    "docx/budget_fy2027_memo.json",
    "odt/battery_storage_overview.json",
    "odt/platform_sync_minutes.json",
    "rtf/aurora_launch_plan.json",
    "xlsx/department_budget_2026.json",
    "xlsx/warehouse_inventory.json",
    "doc/vendor_renewal_letter.json",
];
const OFFICE_DOCUMENT_STEMS: &[&str] = &[
    "lorem_ipsum",
    "docx_tables",
    "unit_test_lists",
    "powerpoint_sample",
    "simple",
    "excel_multi_sheet",
    "headers",
    "formatting",
    "quarterly_operations_report",
    "budget_fy2027_memo",
    "battery_storage_overview",
    "platform_sync_minutes",
    "aurora_launch_plan",
    "department_budget_2026",
    "warehouse_inventory",
    "vendor_renewal_letter",
];
const OFFICE_DOCUMENT_EXTENSIONS: &[&str] = &[
    "docx", "docx", "doc", "pptx", "ppt", "xlsx", "odt", "rtf", "docx", "docx", "odt", "odt", "rtf", "xlsx", "xlsx",
    "doc",
];

const MARKUP_COHORT: &str = "native-markup-fast";
const MARKUP_MANIFEST_NAME: &str = "native-markup-fast-v1";
const MARKUP_MANIFEST_BLAKE3: &str = "f4b40fae95ccf14a0b02e67cacc9e29fa79e27deb8ff81fe98d00170bbdc7264";
const MARKUP_BATCH_SIZE: usize = 8;
const MARKUP_FIXTURES: &[&str] = &[
    "html/complex_table.json",
    "markdown_simple.json",
    "markdown_tables.json",
    "latex/basic_sections.json",
    "typst_simple.json",
    "rst/restructured_text.json",
    "org/tables.json",
    "docbook_chapter.json",
    "html/payments_api_guide.json",
    "latex/cache_layer_release_notes.json",
    "rst/cli_quickstart_tutorial.json",
    "org/wire_protocol_spec.json",
    "docbook/deployment_faq.json",
    "typst/rate_limiter_design.json",
    "md/observability_readme.json",
    "rst/sdk_changelog.json",
];
const MARKUP_DOCUMENT_STEMS: &[&str] = &[
    "complex_table",
    "simple_metadata",
    "tables.pandoc",
    "basic_sections",
    "simple",
    "restructured_text",
    "tables",
    "docbook-chapter",
    "payments_api_guide",
    "cache_layer_release_notes",
    "cli_quickstart_tutorial",
    "wire_protocol_spec",
    "deployment_faq",
    "rate_limiter_design",
    "observability_readme",
    "sdk_changelog",
];
const MARKUP_DOCUMENT_EXTENSIONS: &[&str] = &[
    "html", "md", "md", "tex", "typ", "rst", "org", "docbook", "html", "tex", "rst", "org", "docbook", "typ", "md",
    "rst",
];

const EBOOK_COHORT: &str = "native-ebook-fast";
const EBOOK_MANIFEST_NAME: &str = "native-ebook-fast-v1";
const EBOOK_MANIFEST_BLAKE3: &str = "de4b330755eb815ac33e0e6a171c1d11f1b4c65b46ac548f679f74b379811667";
const EBOOK_BATCH_SIZE: usize = 6;
const EBOOK_FIXTURES: &[&str] = &[
    "epub/simple.json",
    "epub/features.json",
    "epub/wasteland.json",
    "fb2_basic.json",
    "fb2_tables.json",
    "fb2_titles.json",
];
const EBOOK_DOCUMENT_STEMS: &[&str] = &["simple", "features", "wasteland", "basic", "tables", "titles"];
const EBOOK_DOCUMENT_EXTENSIONS: &[&str] = &["epub", "epub", "epub", "fb2", "fb2", "fb2"];

const EMAIL_COHORT: &str = "native-email-fast";
const EMAIL_MANIFEST_NAME: &str = "native-email-fast-v1";
const EMAIL_MANIFEST_BLAKE3: &str = "0bcedb2e4daae6314e05a2ff0c788028707caf12713f176f10f0cc471966f080";
const EMAIL_BATCH_SIZE: usize = 6;
const EMAIL_FIXTURES: &[&str] = &[
    "eml_simple.json",
    "eml_multipart.json",
    "eml/email-no-html-content-1.json",
    "msg_simple.json",
    "msg/fake-email.json",
    "msg_attachments.json",
];
const EMAIL_DOCUMENT_STEMS: &[&str] = &[
    "plain_text_only",
    "multipart_email",
    "email-no-html-content-1",
    "simple_msg",
    "fake-email",
    "msg_with_attachments_alt",
];
const EMAIL_DOCUMENT_EXTENSIONS: &[&str] = &["eml", "eml", "eml", "msg", "msg", "msg"];

const DATA_COHORT: &str = "native-data-fast";
const DATA_MANIFEST_NAME: &str = "native-data-fast-v1";
const DATA_MANIFEST_BLAKE3: &str = "fc2e39e80324501140a7b32f9feba6a593bfec94ecd91e291ca4396ea2abd36f";
const DATA_BATCH_SIZE: usize = 6;
const DATA_FIXTURES: &[&str] = &[
    "csv/csv-comma.json",
    "csv/csv-semicolon.json",
    "tsv_data.json",
    "json_simple.json",
    "json_nested.json",
    "yaml_config.json",
    "csv/quarterly_sales.json",
    "csv/sensor_readings.json",
    "tsv/experiment_results.json",
    "json/api_users.json",
    "json/service_config.json",
    "yaml/deployment.json",
];
const DATA_DOCUMENT_STEMS: &[&str] = &[
    "csv-comma",
    "csv-semicolon",
    "employees",
    "simple",
    "complex_nested",
    "sample_config",
    "quarterly_sales",
    "sensor_readings",
    "experiment_results",
    "api_users",
    "service_config",
    "deployment",
];
const DATA_DOCUMENT_EXTENSIONS: &[&str] = &[
    "csv", "csv", "tsv", "json", "json", "yaml", "csv", "csv", "tsv", "json", "json", "yaml",
];

const IMAGES_COHORT: &str = "ocr-images-fast";
const IMAGES_MANIFEST_NAME: &str = "ocr-images-fast-v1";
const IMAGES_MANIFEST_BLAKE3: &str = "57127121a7d8c03d08d552ce00065436d9f3208b445b00dd704911a5d270b3a0";
const IMAGES_BATCH_SIZE: usize = 14;
const IMAGES_FIXTURES: &[&str] = &[
    "cord_receipt_01.json",
    "cord_receipt_02.json",
    "cord_receipt_03.json",
    "cord_receipt_04.json",
    "doclaynet_page_01.json",
    "doclaynet_page_02.json",
    "ndl_meiji_vertical_01.json",
    "ndl_meiji_vertical_02.json",
    "ndl_meiji_vertical_03.json",
    "ndl_meiji_vertical_04.json",
    "ndl_meiji_vertical_05.json",
    "textocr_scene_01.json",
    "textocr_scene_02.json",
    "textocr_scene_03.json",
];
const IMAGES_DOCUMENT_STEMS: &[&str] = &[
    "cord_receipt_01",
    "cord_receipt_02",
    "cord_receipt_03",
    "cord_receipt_04",
    "doclaynet_page_01",
    "doclaynet_page_02",
    "ndl_meiji_vertical_01",
    "ndl_meiji_vertical_02",
    "ndl_meiji_vertical_03",
    "ndl_meiji_vertical_04",
    "ndl_meiji_vertical_05",
    "textocr_scene_01",
    "textocr_scene_02",
    "textocr_scene_03",
];
const IMAGES_DOCUMENT_EXTENSIONS: &[&str] = &[
    "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg", "jpg",
];

fn matrix_entry(
    artifact: String,
    framework: impl Into<String>,
    output_format: OutputFormat,
    mode: ExecutionMode,
) -> MatrixEntry {
    MatrixEntry {
        artifact,
        framework: framework.into(),
        output_format,
        mode,
        optional: false,
    }
}

/// The Xberg cells for a cohort: markdown/plaintext x single/batch for each enabled pipeline.
/// Rendered-page cohorts (`include_layout`) run both `baseline` and `layout`; OCR cohorts
/// (`include_paddle`) additionally run the PaddleOCR-engine variants `baseline-paddle` (and
/// `layout-paddle` when layout is also included). Sceptre-enabled cohorts add three ORT variants.
/// Each pipeline contributes four cells: the OCR/Image cohorts therefore contain 28 Xberg cells.
fn xberg_entries(cohort: &str, include_layout: bool, include_paddle: bool, include_sceptre: bool) -> Vec<MatrixEntry> {
    let mut pipelines: Vec<&str> = vec!["baseline"];
    if include_layout {
        pipelines.push("layout");
    }
    if include_paddle {
        pipelines.push("baseline-paddle");
        if include_layout {
            pipelines.push("layout-paddle");
        }
    }
    if include_sceptre {
        pipelines.extend(["sceptre-ort", "sceptre-ort-layout", "sceptre-ort-autorotate"]);
    }
    let mut entries = Vec::new();
    for &pipeline in &pipelines {
        for output_format in [OutputFormat::Markdown, OutputFormat::Plaintext] {
            for mode in [ExecutionMode::SingleFile, ExecutionMode::Batch] {
                entries.push(matrix_entry(
                    format!(
                        "benchmarks-rust-{pipeline}-{output_format}-{}-{cohort}",
                        mode.artifact_slug()
                    ),
                    format!("xberg-{output_format}-{pipeline}"),
                    output_format,
                    mode,
                ));
            }
        }
    }
    entries
}

/// The full markdown/plaintext x single/batch grid for one competitor framework.
fn grid_entries(framework: &str, cohort: &str) -> Vec<MatrixEntry> {
    let mut entries = Vec::new();
    for output_format in [OutputFormat::Markdown, OutputFormat::Plaintext] {
        for mode in [ExecutionMode::SingleFile, ExecutionMode::Batch] {
            entries.push(matrix_entry(
                format!(
                    "benchmarks-{framework}-{output_format}-{}-{cohort}",
                    mode.artifact_slug()
                ),
                framework,
                output_format,
                mode,
            ));
        }
    }
    entries
}

/// The single native Markdown/single-file cell for one framework (e.g. MinerU, which never
/// runs in native-batch mode).
fn markdown_single_file_entry(framework: &str, cohort: &str) -> MatrixEntry {
    matrix_entry(
        format!("benchmarks-{framework}-markdown-single-file-{cohort}"),
        framework,
        OutputFormat::Markdown,
        ExecutionMode::SingleFile,
    )
}

/// The single plaintext/single-file cell for one framework (e.g. Tika, Unstructured, whose only
/// benchmarked output is plaintext).
fn plaintext_single_file_entry(framework: &str, cohort: &str) -> MatrixEntry {
    matrix_entry(
        format!("benchmarks-{framework}-plaintext-single-file-{cohort}"),
        framework,
        OutputFormat::Plaintext,
        ExecutionMode::SingleFile,
    )
}

/// Mark every entry best-effort. Competitor cells in the per-format-family cohorts are optional:
/// a framework may not support every extension in a family, and the harness only emits/validates
/// a cell when the framework covers the whole cohort. Absence never fails validation; a present,
/// complete cell still flows into the aggregate.
fn optional(entries: Vec<MatrixEntry>) -> Vec<MatrixEntry> {
    entries.into_iter().map(MatrixEntry::into_optional).collect()
}

fn native_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(NATIVE_COHORT, true, false, false);
    matrix.extend(grid_entries("docling", NATIVE_COHORT));
    matrix.push(matrix_entry(
        format!("benchmarks-markitdown-markdown-single-file-{NATIVE_COHORT}"),
        "markitdown",
        OutputFormat::Markdown,
        ExecutionMode::SingleFile,
    ));
    matrix.push(matrix_entry(
        format!("benchmarks-unstructured-plaintext-single-file-{NATIVE_COHORT}"),
        "unstructured",
        OutputFormat::Plaintext,
        ExecutionMode::SingleFile,
    ));
    matrix.push(matrix_entry(
        format!("benchmarks-tika-plaintext-single-file-{NATIVE_COHORT}"),
        "tika",
        OutputFormat::Plaintext,
        ExecutionMode::SingleFile,
    ));
    matrix.push(matrix_entry(
        format!("benchmarks-pymupdf4llm-markdown-single-file-{NATIVE_COHORT}"),
        "pymupdf4llm",
        OutputFormat::Markdown,
        ExecutionMode::SingleFile,
    ));
    matrix.push(markdown_single_file_entry("mineru", NATIVE_COHORT).into_optional());
    matrix.extend(grid_entries("liteparse", NATIVE_COHORT));
    matrix
}

fn ocr_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(OCR_COHORT, true, true, true);
    matrix.extend(grid_entries("docling", OCR_COHORT));
    matrix.push(markdown_single_file_entry("mineru", OCR_COHORT).into_optional());
    // Tika and Unstructured are Tesseract-backed and now run the OCR cohorts too
    // (single-file, plaintext). Best-effort: absence never fails validation.
    matrix.push(plaintext_single_file_entry("unstructured", OCR_COHORT).into_optional());
    matrix.push(plaintext_single_file_entry("tika", OCR_COHORT).into_optional());
    matrix.extend(grid_entries("liteparse", OCR_COHORT));
    matrix
}

/// Office, markup, and data families share the broad-office competitor set (Docling grid plus the
/// MarkItDown/Unstructured/Tika single-file cells), all best-effort.
fn broad_office_competitors(cohort: &str) -> Vec<MatrixEntry> {
    let mut competitors = optional(grid_entries("docling", cohort));
    competitors.push(markdown_single_file_entry("markitdown", cohort).into_optional());
    competitors.push(plaintext_single_file_entry("unstructured", cohort).into_optional());
    competitors.push(plaintext_single_file_entry("tika", cohort).into_optional());
    competitors
}

fn office_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(OFFICE_COHORT, false, false, false);
    matrix.extend(broad_office_competitors(OFFICE_COHORT));
    matrix
}

fn markup_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(MARKUP_COHORT, false, false, false);
    matrix.extend(broad_office_competitors(MARKUP_COHORT));
    matrix
}

fn data_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(DATA_COHORT, false, false, false);
    matrix.extend(broad_office_competitors(DATA_COHORT));
    matrix
}

fn ebook_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(EBOOK_COHORT, false, false, false);
    matrix.push(plaintext_single_file_entry("tika", EBOOK_COHORT).into_optional());
    matrix.push(markdown_single_file_entry("pymupdf4llm", EBOOK_COHORT).into_optional());
    matrix
}

fn email_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(EMAIL_COHORT, false, false, false);
    matrix.push(plaintext_single_file_entry("unstructured", EMAIL_COHORT).into_optional());
    matrix.push(plaintext_single_file_entry("tika", EMAIL_COHORT).into_optional());
    matrix
}

fn images_matrix() -> Vec<MatrixEntry> {
    let mut matrix = xberg_entries(IMAGES_COHORT, true, true, true);
    matrix.extend(optional(grid_entries("docling", IMAGES_COHORT)));
    // pymupdf4llm has no OCR path (`pymupdf4llm.to_markdown` only reads existing text layers), so on
    // a scanned-image cohort it always returns empty content and can never clear the min-success-rate
    // gate — it is excluded here to avoid a guaranteed-failing best-effort cell. It still runs the
    // PDF/ebook cohorts, where a text layer exists.
    matrix.push(markdown_single_file_entry("mineru", IMAGES_COHORT).into_optional());
    // Tika and Unstructured are Tesseract-backed and now run the image OCR cohort too
    // (single-file, plaintext). Best-effort: absence never fails validation.
    matrix.push(plaintext_single_file_entry("unstructured", IMAGES_COHORT).into_optional());
    matrix.push(plaintext_single_file_entry("tika", IMAGES_COHORT).into_optional());
    matrix
}

fn native_contract() -> CohortContract {
    CohortContract {
        manifest_name: NATIVE_MANIFEST_NAME,
        manifest_blake3: NATIVE_MANIFEST_BLAKE3,
        batch_size: NATIVE_BATCH_SIZE,
        fixtures: NATIVE_FIXTURES,
        document_stems: NATIVE_DOCUMENT_STEMS,
        document_extensions: NATIVE_DOCUMENT_EXTENSIONS,
        matrix: native_matrix(),
    }
}

fn ocr_contract() -> CohortContract {
    CohortContract {
        manifest_name: OCR_MANIFEST_NAME,
        manifest_blake3: OCR_MANIFEST_BLAKE3,
        batch_size: OCR_BATCH_SIZE,
        fixtures: OCR_FIXTURES,
        document_stems: OCR_DOCUMENT_STEMS,
        document_extensions: OCR_DOCUMENT_EXTENSIONS,
        matrix: ocr_matrix(),
    }
}

fn office_contract() -> CohortContract {
    CohortContract {
        manifest_name: OFFICE_MANIFEST_NAME,
        manifest_blake3: OFFICE_MANIFEST_BLAKE3,
        batch_size: OFFICE_BATCH_SIZE,
        fixtures: OFFICE_FIXTURES,
        document_stems: OFFICE_DOCUMENT_STEMS,
        document_extensions: OFFICE_DOCUMENT_EXTENSIONS,
        matrix: office_matrix(),
    }
}

fn markup_contract() -> CohortContract {
    CohortContract {
        manifest_name: MARKUP_MANIFEST_NAME,
        manifest_blake3: MARKUP_MANIFEST_BLAKE3,
        batch_size: MARKUP_BATCH_SIZE,
        fixtures: MARKUP_FIXTURES,
        document_stems: MARKUP_DOCUMENT_STEMS,
        document_extensions: MARKUP_DOCUMENT_EXTENSIONS,
        matrix: markup_matrix(),
    }
}

fn ebook_contract() -> CohortContract {
    CohortContract {
        manifest_name: EBOOK_MANIFEST_NAME,
        manifest_blake3: EBOOK_MANIFEST_BLAKE3,
        batch_size: EBOOK_BATCH_SIZE,
        fixtures: EBOOK_FIXTURES,
        document_stems: EBOOK_DOCUMENT_STEMS,
        document_extensions: EBOOK_DOCUMENT_EXTENSIONS,
        matrix: ebook_matrix(),
    }
}

fn email_contract() -> CohortContract {
    CohortContract {
        manifest_name: EMAIL_MANIFEST_NAME,
        manifest_blake3: EMAIL_MANIFEST_BLAKE3,
        batch_size: EMAIL_BATCH_SIZE,
        fixtures: EMAIL_FIXTURES,
        document_stems: EMAIL_DOCUMENT_STEMS,
        document_extensions: EMAIL_DOCUMENT_EXTENSIONS,
        matrix: email_matrix(),
    }
}

fn data_contract() -> CohortContract {
    CohortContract {
        manifest_name: DATA_MANIFEST_NAME,
        manifest_blake3: DATA_MANIFEST_BLAKE3,
        batch_size: DATA_BATCH_SIZE,
        fixtures: DATA_FIXTURES,
        document_stems: DATA_DOCUMENT_STEMS,
        document_extensions: DATA_DOCUMENT_EXTENSIONS,
        matrix: data_matrix(),
    }
}

fn images_contract() -> CohortContract {
    CohortContract {
        manifest_name: IMAGES_MANIFEST_NAME,
        manifest_blake3: IMAGES_MANIFEST_BLAKE3,
        batch_size: IMAGES_BATCH_SIZE,
        fixtures: IMAGES_FIXTURES,
        document_stems: IMAGES_DOCUMENT_STEMS,
        document_extensions: IMAGES_DOCUMENT_EXTENSIONS,
        matrix: images_matrix(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// (cohort, total cells, required (non-optional) cells). The required count is what the raw
    /// artifact / aggregate validators gate publication on; optional competitor cells are allowed
    /// but never required for the per-format-family cohorts.
    const CONTRACT_CELL_COUNTS: [(Cohort, usize, usize); 8] = [
        (Cohort::Native, 21, 20),
        (Cohort::Ocr, 39, 36),
        (Cohort::Office, 11, 4),
        (Cohort::Markup, 11, 4),
        (Cohort::Ebook, 6, 4),
        (Cohort::Email, 6, 4),
        (Cohort::Data, 11, 4),
        (Cohort::Images, 35, 28),
    ];

    #[test]
    fn contract_cell_counts_are_exact_and_uniquely_keyed() {
        for (cohort, total, required) in CONTRACT_CELL_COUNTS {
            let contract = cohort.contract();
            assert_eq!(contract.matrix.len(), total, "{} total cells", cohort.as_str());
            assert_eq!(
                contract.matrix.iter().filter(|entry| !entry.optional).count(),
                required,
                "{} required cells",
                cohort.as_str()
            );
            assert_eq!(
                contract
                    .matrix
                    .iter()
                    .map(|entry| &entry.artifact)
                    .collect::<HashSet<_>>()
                    .len(),
                contract.matrix.len(),
                "{} artifact names must be unique",
                cohort.as_str()
            );
            assert_eq!(
                contract
                    .matrix
                    .iter()
                    .map(MatrixEntry::aggregate_key)
                    .collect::<HashSet<_>>()
                    .len(),
                contract.matrix.len(),
                "{} aggregate keys must be unique",
                cohort.as_str()
            );
        }
    }

    #[test]
    fn only_rendered_page_cohorts_run_the_layout_pipeline() {
        for cohort in Cohort::ALL {
            let contract = cohort.contract();
            let has_layout = contract
                .matrix
                .iter()
                .any(|entry| entry.framework.starts_with("xberg-") && entry.framework.ends_with("-layout"));
            assert_eq!(
                has_layout,
                cohort.includes_layout(),
                "{}: layout-pipeline presence must match includes_layout()",
                cohort.as_str()
            );
            let has_paddle = contract
                .matrix
                .iter()
                .any(|entry| entry.framework.starts_with("xberg-") && entry.framework.ends_with("-paddle"));
            assert_eq!(
                has_paddle,
                cohort.includes_paddle_ocr(),
                "{}: paddle-pipeline presence must match includes_paddle_ocr()",
                cohort.as_str()
            );
            let has_sceptre = contract
                .matrix
                .iter()
                .any(|entry| entry.framework.contains("-sceptre-ort"));
            assert_eq!(
                has_sceptre,
                cohort.includes_sceptre_ort(),
                "{}: Sceptre ORT presence must match includes_sceptre_ort()",
                cohort.as_str()
            );
            let xberg_required = contract
                .matrix
                .iter()
                .filter(|entry| entry.framework.starts_with("xberg-"))
                .count();
            // 4 cells (md/plain x single/batch) per enabled pipeline: baseline, +layout,
            // +baseline-paddle, +layout-paddle (the last only when layout is also present).
            let mut pipelines: usize = 1;
            if cohort.includes_layout() {
                pipelines += 1;
            }
            if cohort.includes_paddle_ocr() {
                pipelines += 1;
                if cohort.includes_layout() {
                    pipelines += 1;
                }
            }
            if cohort.includes_sceptre_ort() {
                pipelines += 3;
            }
            assert_eq!(xberg_required, pipelines * 4, "{}: xberg cell count", cohort.as_str());
        }
    }

    #[test]
    fn ocr_release_cohorts_include_explicit_ort_variants_but_not_tract() {
        for cohort in [Cohort::Ocr, Cohort::Images] {
            let contract = cohort.contract();
            let frameworks: HashSet<&str> = contract.matrix.iter().map(|entry| entry.framework.as_str()).collect();
            for variant in ["sceptre-ort", "sceptre-ort-layout", "sceptre-ort-autorotate"] {
                assert!(
                    frameworks.iter().any(|framework| framework.ends_with(variant)),
                    "{} missing {variant}",
                    cohort.as_str()
                );
            }
            assert!(!frameworks.iter().any(|framework| framework.contains("sceptre-tract")));
        }
    }

    #[test]
    fn sceptre_ort_aggregate_keys_preserve_engine_and_variant_identity() {
        let contract = Cohort::Ocr.contract();
        let keys: HashSet<String> = contract.matrix.iter().map(MatrixEntry::aggregate_key).collect();
        for variant in ["sceptre-ort", "sceptre-ort-layout", "sceptre-ort-autorotate"] {
            assert!(keys.contains(&format!("xberg-markdown-{variant}:single")));
            assert!(keys.contains(&format!("xberg-plaintext-{variant}:batch")));
        }
        assert!(keys.iter().all(|key| !key.contains("sceptre-tract")));
    }

    #[test]
    fn mineru_is_single_file_only_wherever_present() {
        for cohort in Cohort::ALL {
            let contract = cohort.contract();
            let mineru_modes: Vec<ExecutionMode> = contract
                .matrix
                .iter()
                .filter(|entry| entry.framework == "mineru")
                .map(|entry| entry.mode)
                .collect();
            assert!(
                mineru_modes.iter().all(|mode| *mode == ExecutionMode::SingleFile),
                "{}: mineru must be single-file only",
                cohort.as_str()
            );
        }
    }

    #[test]
    fn cohort_parse_round_trips_as_str() {
        for cohort in Cohort::ALL {
            assert_eq!(Cohort::parse(cohort.as_str()).unwrap(), cohort);
        }
        assert!(Cohort::parse("bogus").is_err());
    }

    #[test]
    fn xberg_aggregate_keys_omit_output_format() {
        let contract = Cohort::Native.contract();
        let entry = contract
            .matrix
            .iter()
            .find(|entry| entry.framework == "xberg-markdown-baseline")
            .expect("xberg-markdown-baseline cell present");
        assert_eq!(entry.aggregate_key(), "xberg-markdown-baseline:single");
    }
}
