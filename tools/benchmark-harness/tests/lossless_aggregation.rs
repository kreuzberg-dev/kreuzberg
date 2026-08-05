//! Round-trip lossless-aggregation regression test (B2).
//!
//! An aggregation audit (see the B2 oracle in the benchmark-harness release notes) found that
//! `consolidate` silently dropped the entire `provenance.json` sidecar and roughly fifteen
//! measured fields from every `results.json` row. This test builds a synthetic
//! `results.json` + `provenance.json` pair on disk exactly as the `run` command would, loads them
//! through the same `load_run_results`/`load_run_provenance` + `aggregate_new_format` path the
//! `consolidate` CLI command uses, and asserts every one of those fields survives into
//! `aggregated.json` unchanged. It mirrors the oracle's `jq` presence checks as Rust assertions.

use benchmark_harness::provenance::{
    CorpusProvenance, ExecutableProvenance, FixtureProvenance, FrameworkProvenance, RepositoryProvenance,
    RunProvenance, TimingProvenance,
};
use benchmark_harness::system_load::SystemLoad;
use benchmark_harness::types::{
    BatchCapability, BatchEntryPoint, BatchTimingScope, BenchmarkResult, ErrorKind, FrameworkCapabilities,
    IterationResult, OcrStatus, OutputFormat, PdfMetadata, PerformanceMetrics, QualityMetrics,
};
use benchmark_harness::{
    aggregate_new_format, load_run_provenance, load_run_results, write_json, write_run_provenance,
};

use std::path::PathBuf;
use std::time::Duration;

fn sample_metrics() -> PerformanceMetrics {
    PerformanceMetrics {
        baseline_memory_bytes: 12_582_912,
        peak_memory_bytes: 188_743_680,
        peak_memory_delta_bytes: 176_160_768,
        avg_cpu_percent: 42.1,
        cpu_seconds: 0.31,
        throughput_bytes_per_sec: 361_280.5,
        p50_memory_bytes: 150_000_000,
        p95_memory_bytes: 175_000_000,
        p99_memory_bytes: 189_000_000,
    }
}

fn successful_result() -> BenchmarkResult {
    BenchmarkResult {
        framework: "framework-x".to_string(),
        output_format: OutputFormat::Markdown,
        file_path: PathBuf::from("fixture_one.pdf"),
        file_size: 45_210,
        success: true,
        error_message: None,
        error_kind: ErrorKind::None,
        duration: Duration::from_millis(125),
        extraction_duration: Some(Duration::from_millis(98)),
        subprocess_overhead: Some(Duration::from_millis(27)),
        metrics: sample_metrics(),
        quality: Some(QualityMetrics {
            f1_score_text: 0.92,
            f1_score_numeric: 0.85,
            f1_score_layout: Some(0.78),
            quality_score: 0.85,
            missing_tokens: vec![("invoice".to_string(), 2)],
            extra_tokens: vec![("garble".to_string(), 1)],
            correct: true,
        }),
        iterations: vec![IterationResult {
            iteration: 0,
            duration: Duration::from_millis(125),
            extraction_duration: Some(Duration::from_millis(98)),
            metrics: sample_metrics(),
        }],
        statistics: None,
        cold_start_duration: Some(Duration::from_millis(210)),
        file_extension: "pdf".to_string(),
        framework_capabilities: FrameworkCapabilities {
            supported_extensions: vec!["pdf".to_string(), "docx".to_string()],
            ocr_support: true,
            batch_support: true,
            batch_capability: Some(BatchCapability {
                entry_point: BatchEntryPoint::XbergCliExtractBatch,
                timing_scope: BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: true,
            }),
            batch_performance_sample: None,
            batch_sample_id: None,
            async_support: true,
            supported_output_formats: vec![OutputFormat::Markdown, OutputFormat::Plaintext],
            version: "9.9.9".to_string(),
            installation_size: None,
        },
        pdf_metadata: Some(PdfMetadata {
            has_text_layer: true,
            detection_method: "pdftotext".to_string(),
            page_count: Some(3),
            ocr_enabled: false,
            text_quality_score: Some(0.6),
        }),
        ocr_status: OcrStatus::NotUsed,
        extracted_text: None,
        system_load: Some(SystemLoad {
            load_avg_1m: 1.5,
            load_avg_5m: 2.5,
            load_avg_15m: 3.5,
            logical_cores: 8,
            physical_cores: 4,
        }),
    }
}

fn failed_result() -> BenchmarkResult {
    let mut result = successful_result();
    result.file_path = PathBuf::from("fixture_two.pdf");
    result.success = false;
    result.error_message = Some("framework subprocess exited with code 137".to_string());
    result.error_kind = ErrorKind::HarnessError;
    result.quality = None;
    result.metrics.throughput_bytes_per_sec = 0.0;
    result
}

fn sample_provenance() -> RunProvenance {
    RunProvenance {
        schema_version: 2,
        harness_version: "1.0.0-rc.41".to_string(),
        repository: RepositoryProvenance {
            commit: Some("ea739f5ed845a8f787af203e1f8f7b1f3e0a436e".to_string()),
            dirty: Some(true),
        },
        corpus: CorpusProvenance {
            cohort: Some("native-pdf-fast-b8-v1".to_string()),
            cohort_manifest_blake3: Some(
                "c10d1f78d3f9d61070c0d91e7bfe90a904b69c9d3261536eb1dcff2081c73f6b".to_string(),
            ),
            ordered_fixtures: vec![FixtureProvenance {
                fixture: "pdf_tiny_memo.json".to_string(),
                fixture_blake3: "2f71c153035264ec996898db2926cb8e6b7f8cd39550b62b4cf26ee991752aa4".to_string(),
                document_blake3: "94407260fa7da9d342c39fcfe620260d4a5ef8bc3868f44f5baa600c0e14f498".to_string(),
                document_bytes: 13_374,
            }],
        },
        frameworks: vec![FrameworkProvenance {
            name: "framework-x".to_string(),
            version: "9.9.9".to_string(),
            executable: Some(ExecutableProvenance {
                name: "framework-x-cli".to_string(),
                blake3: Some("a".repeat(64)),
                invocation_blake3: "b".repeat(64),
            }),
            models: vec!["org/model@rev#deadbeef".to_string()],
            batch_capability: Some(BatchCapability {
                entry_point: BatchEntryPoint::XbergCliExtractBatch,
                timing_scope: BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: true,
            }),
            requested_workers: Some(4),
            effective_workers: Some(4),
            configured_thread_budget: Some(8),
            worker_semantics: "configured document concurrency cap; Xberg thread budget is recorded separately"
                .to_string(),
            effective_warmup_iterations: 1,
            eligible_documents: 8,
            batch_partitions: Some(1),
            ocr_language_policy: benchmark_harness::adapter::OcrLanguagePolicy::DefaultOnly,
        }],
        timing: TimingProvenance {
            mode: benchmark_harness::BenchmarkMode::Batch,
            warmup_iterations: 1,
            benchmark_iterations: 3,
            timeout_ms: 900_000,
            output_format: OutputFormat::Markdown,
        },
        fixed_batch_size: Some(8),
    }
}

/// Writes a synthetic `results.json` + `provenance.json` pair under `root/framework-x-batch/`,
/// exactly as the `run` command lays out one artifact directory.
fn materialize_run_directory(root: &std::path::Path) -> PathBuf {
    let run_dir = root.join("framework-x-batch");
    write_json(&[successful_result(), failed_result()], &run_dir.join("results.json")).expect("write results.json");
    write_run_provenance(&sample_provenance(), &run_dir.join("provenance.json")).expect("write provenance.json");
    run_dir
}

#[test]
fn consolidate_pipeline_preserves_every_provenance_field() {
    let root = tempfile::tempdir().expect("tempdir");
    materialize_run_directory(root.path());

    let results = load_run_results(root.path()).expect("load results");
    let provenance_records = load_run_provenance(root.path()).expect("load provenance");
    let mut aggregated = aggregate_new_format(&results);
    aggregated.run_provenance = provenance_records;

    assert_eq!(aggregated.run_provenance.len(), 1);
    let record = &aggregated.run_provenance[0];
    assert!(record.missing_reason.is_none());
    let provenance = record.provenance.as_ref().expect("provenance parsed");

    // Oracle loss point 1: repository commit/dirty.
    assert_eq!(
        provenance.repository.commit.as_deref(),
        Some("ea739f5ed845a8f787af203e1f8f7b1f3e0a436e")
    );
    assert_eq!(provenance.repository.dirty, Some(true));

    // Corpus cohort + fixture BLAKE3s.
    assert_eq!(provenance.corpus.cohort.as_deref(), Some("native-pdf-fast-b8-v1"));
    assert_eq!(
        provenance.corpus.cohort_manifest_blake3.as_deref(),
        Some("c10d1f78d3f9d61070c0d91e7bfe90a904b69c9d3261536eb1dcff2081c73f6b")
    );
    assert_eq!(provenance.corpus.ordered_fixtures.len(), 1);
    assert_eq!(provenance.corpus.ordered_fixtures[0].fixture, "pdf_tiny_memo.json");
    assert_eq!(provenance.corpus.ordered_fixtures[0].document_bytes, 13_374);

    // Framework version/executable/models/worker counts/batch_capability.
    assert_eq!(provenance.frameworks.len(), 1);
    let framework = &provenance.frameworks[0];
    assert_eq!(framework.version, "9.9.9");
    assert_eq!(framework.executable.as_ref().unwrap().name, "framework-x-cli");
    assert_eq!(framework.models, vec!["org/model@rev#deadbeef".to_string()]);
    assert_eq!(framework.requested_workers, Some(4));
    assert_eq!(framework.effective_workers, Some(4));
    assert_eq!(framework.configured_thread_budget, Some(8));
    assert!(framework.batch_capability.is_some());

    // Timing config + fixed_batch_size.
    assert_eq!(provenance.timing.timeout_ms, 900_000);
    assert_eq!(provenance.timing.output_format, OutputFormat::Markdown);
    assert_eq!(provenance.fixed_batch_size, Some(8));

    // Round-trips through JSON serialization exactly as `aggregated.json` would be written.
    let json = serde_json::to_value(&aggregated).expect("serialize aggregate");
    assert_eq!(
        json["run_provenance"][0]["provenance"]["repository"]["commit"],
        "ea739f5ed845a8f787af203e1f8f7b1f3e0a436e"
    );
}

#[test]
fn consolidate_pipeline_records_missing_provenance_without_failing() {
    let root = tempfile::tempdir().expect("tempdir");
    let run_dir = root.path().join("framework-y-single");
    write_json(&[successful_result()], &run_dir.join("results.json")).expect("write results.json");
    // Deliberately no provenance.json written.

    let provenance_records = load_run_provenance(root.path()).expect("load provenance");

    assert_eq!(provenance_records.len(), 1);
    assert!(provenance_records[0].provenance.is_none());
    assert!(
        provenance_records[0]
            .missing_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("provenance.json not found"))
    );
}

#[test]
fn consolidate_pipeline_preserves_every_per_fixture_field_for_a_successful_result() {
    let root = tempfile::tempdir().expect("tempdir");
    materialize_run_directory(root.path());

    let results = load_run_results(root.path()).expect("load results");
    let aggregated = aggregate_new_format(&results);

    let row = aggregated
        .per_fixture_results
        .iter()
        .find(|row| row.fixture_id == "fixture_one")
        .expect("fixture_one row present");

    // Oracle loss point 2/3/4/6: per-result fields previously dropped from PerFixtureRow.
    assert_eq!(row.file_size, 45_210);
    assert!((row.throughput_bytes_per_sec - 361_280.5).abs() < f64::EPSILON);
    assert_eq!(row.avg_cpu_percent, 42.1);
    assert_eq!(row.cpu_seconds, 0.31);
    assert_eq!(row.baseline_memory_bytes, 12_582_912);
    assert_eq!(row.peak_memory_delta_bytes, 176_160_768);
    assert_eq!(row.p50_memory_bytes, 150_000_000);
    assert_eq!(row.p95_memory_bytes, 175_000_000);
    assert_eq!(row.p99_memory_bytes, 189_000_000);
    assert_eq!(row.extraction_duration_ms, Some(98.0));
    assert_eq!(row.subprocess_overhead_ms, Some(27.0));
    assert_eq!(row.cold_start_duration_ms, Some(210.0));
    assert_eq!(row.error_message, None);

    let quality = row.quality.as_ref().expect("quality present");
    assert_eq!(row.f1_text, Some(0.92));
    assert_eq!(row.f1_layout, Some(0.78));
    assert_eq!(row.f1_numeric, Some(0.85));
    assert_eq!(quality.f1_score_text, 0.92);
    assert_eq!(quality.f1_score_layout, Some(0.78));
    assert_eq!(quality.missing_tokens, vec![("invoice".to_string(), 2)]);
    assert_eq!(quality.extra_tokens, vec![("garble".to_string(), 1)]);

    let pdf_metadata = row.pdf_metadata.as_ref().expect("pdf_metadata present");
    assert!(pdf_metadata.has_text_layer);
    assert_eq!(pdf_metadata.detection_method, "pdftotext");
    assert!(!pdf_metadata.ocr_enabled);
    assert_eq!(pdf_metadata.text_quality_score, Some(0.6));

    // Oracle loss point 3/4: framework_capabilities.{version,supported_extensions,ocr_support,
    // async_support,supported_output_formats,batch_capability}.
    assert_eq!(row.framework_capabilities.version, "9.9.9");
    assert_eq!(
        row.framework_capabilities.supported_extensions,
        vec!["pdf".to_string(), "docx".to_string()]
    );
    assert!(row.framework_capabilities.ocr_support);
    assert!(row.framework_capabilities.async_support);
    assert_eq!(
        row.framework_capabilities.supported_output_formats,
        vec![OutputFormat::Markdown, OutputFormat::Plaintext]
    );
    let batch_capability = row
        .framework_capabilities
        .batch_capability
        .expect("batch_capability present");
    assert_eq!(batch_capability.entry_point, BatchEntryPoint::XbergCliExtractBatch);

    // Oracle loss point 3: system_load raw fields (load_avg_*, logical/physical_cores).
    let system_load = row.system_load.expect("system_load present");
    assert_eq!(system_load.load_avg_1m, 1.5);
    assert_eq!(system_load.load_avg_5m, 2.5);
    assert_eq!(system_load.load_avg_15m, 3.5);
    assert_eq!(system_load.logical_cores, 8);
    assert_eq!(system_load.physical_cores, 4);

    // Oracle loss point 2: iterations[]/statistics.
    assert_eq!(row.iterations.len(), 1);
    assert_eq!(row.iterations[0].iteration, 0);
    assert!(row.statistics.is_none());
}

#[test]
fn consolidate_pipeline_preserves_error_message_for_a_failed_result() {
    let root = tempfile::tempdir().expect("tempdir");
    materialize_run_directory(root.path());

    let results = load_run_results(root.path()).expect("load results");
    let aggregated = aggregate_new_format(&results);

    let row = aggregated
        .per_fixture_results
        .iter()
        .find(|row| row.fixture_id == "fixture_two")
        .expect("fixture_two row present");

    // Oracle loss point 5: error_message free text, not just the coarse error_kind enum.
    assert!(!row.success);
    assert_eq!(row.error_kind.as_deref(), Some("HarnessError"));
    assert_eq!(
        row.error_message.as_deref(),
        Some("framework subprocess exited with code 137")
    );
}

/// Oracle loss point 7: the batch performance-sample dedup that collapses a native batch to one
/// `performance_sample_count` must not also collapse `per_fixture_results` — every per-document
/// row (including the failed one) must survive.
#[test]
fn consolidate_pipeline_preserves_every_batch_document_row() {
    let root = tempfile::tempdir().expect("tempdir");
    materialize_run_directory(root.path());

    let results = load_run_results(root.path()).expect("load results");
    let aggregated = aggregate_new_format(&results);

    // The batch heuristic tags this directory's framework as "framework-x-batch"; consolidate's
    // mode-extraction normalizes it back to "framework-x" + mode "batch".
    let batch_rows: Vec<_> = aggregated
        .per_fixture_results
        .iter()
        .filter(|row| row.framework == "framework-x" && row.execution_mode == "batch")
        .collect();
    assert_eq!(batch_rows.len(), 2, "both batch documents must survive as rows");

    let fixture_ids: std::collections::HashSet<&str> = batch_rows.iter().map(|row| row.fixture_id.as_str()).collect();
    assert_eq!(
        fixture_ids,
        std::collections::HashSet::from(["fixture_one", "fixture_two"])
    );
}
