//! Integration tests for the benchmark artifact/aggregate release-contract validator.
//!
//! Ported from `tools/benchmark-harness/tests/test_validate_benchmark_artifacts.py`, which
//! exercised `scripts/ci/benchmarks/validate-benchmark-artifacts.py`. Real cohort manifests and
//! fixture descriptors are copied from the repository (so a pinned-digest drift is caught), but
//! referenced documents are synthetic — the validator never reads document content, only its
//! BLAKE3 digest and byte length, which the synthetic bytes provide consistently.

use benchmark_harness::aggregate::{FileTypeAggregation, NewConsolidatedResults, RankedFramework};
use benchmark_harness::bench_matrix::{Cohort, CohortContract};
use benchmark_harness::consolidate::RunProvenanceRecord;
use benchmark_harness::provenance::{
    CorpusProvenance, FixtureProvenance, FrameworkProvenance, RepositoryProvenance, RunProvenance, TimingProvenance,
};
use benchmark_harness::types::{
    BenchmarkResult, ErrorKind, FrameworkCapabilities, IterationResult, OcrStatus, OutputFormat, PerformanceMetrics,
    QualityMetrics,
};
use benchmark_harness::validate_artifacts::{ValidateArtifactsArgs, validate};
use benchmark_harness::{Error, write_json, write_run_provenance};

use std::path::{Path, PathBuf};
use std::time::Duration;
use tempfile::TempDir;

const SOURCE_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const RUN_ID: &str = "42";
const ITERATIONS: usize = 3;

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn blake3_hex(path: &Path) -> String {
    blake3::hash(&std::fs::read(path).expect("read file to hash"))
        .to_hex()
        .to_string()
}

fn zero_metrics() -> PerformanceMetrics {
    PerformanceMetrics {
        baseline_memory_bytes: 0,
        peak_memory_bytes: 0,
        peak_memory_delta_bytes: 0,
        avg_cpu_percent: 0.0,
        cpu_seconds: 0.0,
        throughput_bytes_per_sec: 0.0,
        p50_memory_bytes: 0,
        p95_memory_bytes: 0,
        p99_memory_bytes: 0,
    }
}

/// A cohort's fixture tree, materialized under a temp root exactly like the release CI layout:
/// real cohort manifest + fixture descriptor bytes (so pinned digests stay honest), synthetic
/// document bytes (the validator never reads document content).
struct FixtureTree {
    _root: TempDir,
    cohort_manifest: PathBuf,
    fixtures_root: PathBuf,
    manifest_blake3: String,
    fixture_provenance: Vec<FixtureProvenance>,
}

fn copy_ground_truth_files(source_descriptor: &Path, descriptor: &Path, value: &serde_json::Value) {
    let source_parent = source_descriptor.parent().expect("source descriptor has a parent");
    let destination_parent = descriptor.parent().expect("descriptor has a parent");
    for field in ["text_file", "markdown_file"] {
        let Some(relative_path) = value["ground_truth"][field].as_str() else {
            continue;
        };
        let destination = destination_parent.join(relative_path);
        std::fs::create_dir_all(destination.parent().expect("ground truth has a parent"))
            .expect("create ground truth dir");
        std::fs::copy(source_parent.join(relative_path), destination).expect("copy ground truth");
    }
}

fn materialize_fixture_tree(cohort: Cohort, contract: &CohortContract) -> FixtureTree {
    let root = tempfile::tempdir().expect("tempdir");
    let cohort_manifest = root.path().join("cohort.json");
    let harness_root = root.path().join("tools/benchmark-harness");
    let fixtures_root = harness_root.join("fixtures");
    std::fs::create_dir_all(&fixtures_root).expect("create fixtures root");
    // ~keep Materialize the repository anchor used by production fixture validation so these
    // release-layout tests exercise their intended contract instead of a standalone-tree boundary.
    std::fs::copy(repo_path("Cargo.toml"), harness_root.join("Cargo.toml")).expect("copy harness manifest");

    let cohort_slug = match cohort {
        Cohort::Native => "native-pdf-fast-b8",
        Cohort::Ocr => "ocr-pdf-fast-b4",
        Cohort::Office => "native-office-fast",
        Cohort::Markup => "native-markup-fast",
        Cohort::Ebook => "native-ebook-fast",
        Cohort::Email => "native-email-fast",
        Cohort::Data => "native-data-fast",
        Cohort::Images => "ocr-images-fast",
    };
    std::fs::copy(repo_path(&format!("cohorts/{cohort_slug}.json")), &cohort_manifest).expect("copy cohort manifest");
    let manifest_blake3 = blake3_hex(&cohort_manifest);

    let mut fixture_provenance = Vec::with_capacity(contract.fixtures.len());
    for (fixture, document_stem) in contract.fixtures.iter().zip(contract.document_stems.iter()) {
        let descriptor_path = fixtures_root.join(fixture);
        let source_descriptor_path = repo_path(&format!("fixtures/{fixture}"));
        std::fs::create_dir_all(descriptor_path.parent().expect("descriptor has a parent"))
            .expect("create descriptor dir");
        std::fs::copy(&source_descriptor_path, &descriptor_path).expect("copy fixture descriptor");

        let descriptor: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&descriptor_path).expect("read descriptor"))
                .expect("parse descriptor");
        // ~keep Production validates descriptor-owned ground truth before artifact contracts.
        // Copy the checked-in bytes so this synthetic repository exercises the same boundary.
        copy_ground_truth_files(&source_descriptor_path, &descriptor_path, &descriptor);
        let document_field = descriptor["document"]
            .as_str()
            .expect("descriptor has a document field");
        let document_path = descriptor_path
            .parent()
            .expect("descriptor has a parent")
            .join(document_field);
        std::fs::create_dir_all(document_path.parent().expect("document has a parent")).expect("create document dir");
        std::fs::write(
            &document_path,
            format!("temporary benchmark document: {document_stem}\n"),
        )
        .expect("write document");

        fixture_provenance.push(FixtureProvenance {
            fixture: (*fixture).to_string(),
            fixture_blake3: blake3_hex(&descriptor_path),
            document_blake3: blake3_hex(&document_path),
            document_bytes: std::fs::metadata(&document_path).expect("stat document").len(),
        });
    }

    FixtureTree {
        _root: root,
        cohort_manifest,
        fixtures_root,
        manifest_blake3,
        fixture_provenance,
    }
}

/// The framework name the runner actually writes into artifacts. Xberg batch cells carry a
/// `-batch` suffix (see `adapters::xberg` and `normalize_run_frameworks`); competitors and
/// single-file cells use the bare name. Mirroring this here keeps the fixtures faithful to real
/// artifacts so the validator's suffix handling is exercised.
fn runtime_framework_name(entry: &benchmark_harness::bench_matrix::MatrixEntry) -> String {
    use benchmark_harness::bench_matrix::ExecutionMode;

    if matches!(entry.mode, ExecutionMode::Batch) && entry.framework.starts_with("xberg-") {
        format!("{}-batch", entry.framework)
    } else {
        entry.framework.clone()
    }
}

fn aggregate_framework_name(entry: &benchmark_harness::bench_matrix::MatrixEntry) -> String {
    let mut framework = runtime_framework_name(entry);
    if matches!(entry.mode, benchmark_harness::bench_matrix::ExecutionMode::Batch) && !framework.ends_with("-batch") {
        framework.push_str("-batch");
    }
    framework
}

fn supports_extension(framework: &str, extension: &str) -> bool {
    if framework.starts_with("xberg-") {
        return true;
    }
    let supported: &[&str] = match framework {
        "liteparse" => &["pdf"],
        "pymupdf4llm" => &["pdf", "epub", "fb2", "png", "jpg", "jpeg", "bmp", "tiff", "tif"],
        "docling" => &[
            "pdf", "docx", "pptx", "xlsx", "html", "md", "csv", "png", "jpg", "jpeg", "tiff", "tif", "bmp",
        ],
        "tika" => &[
            "pdf", "docx", "doc", "pptx", "ppt", "xlsx", "odt", "rtf", "epub", "html", "md", "csv", "tsv", "json",
            "yaml", "eml", "msg", "tex", "rst", "org", "png", "jpg", "jpeg", "tiff", "tif",
        ],
        "markitdown" => &[
            "pdf", "docx", "pptx", "xlsx", "html", "md", "csv", "json", "epub", "msg", "png", "jpg", "jpeg", "bmp",
            "tiff", "tif",
        ],
        "unstructured" => &[
            "pdf", "docx", "doc", "pptx", "ppt", "xlsx", "odt", "rtf", "epub", "html", "md", "rst", "org", "csv",
            "tsv", "eml", "msg", "png", "jpg", "jpeg", "tiff", "tif", "bmp",
        ],
        "mineru" => &["pdf", "png", "jpg", "jpeg", "bmp", "tiff", "tif"],
        _ => &[],
    };
    supported.contains(&extension)
}

fn supports_fixture_language(framework: &str, fixture: &str) -> bool {
    let descriptor: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo_path(&format!("fixtures/{fixture}"))).expect("read fixture descriptor"),
    )
    .expect("parse fixture descriptor");
    let language = descriptor["metadata"]["ocr_language"].as_str();
    benchmark_harness::adapter::declared_ocr_language_policy(framework).supports(language)
}

#[test]
fn ocr_pdf_contract_eligibility_matches_framework_language_capabilities() {
    let contract = Cohort::Ocr.contract();
    let eligible = |framework: &str| {
        contract
            .document_extensions
            .iter()
            .zip(contract.fixtures.iter())
            .filter(|(extension, fixture)| {
                supports_extension(framework, extension) && supports_fixture_language(framework, fixture)
            })
            .count()
    };

    assert_eq!(
        eligible("liteparse"),
        3,
        "default-language LiteParse must exclude German"
    );
    assert_eq!(eligible("docling"), 4, "Docling accepts the per-batch German selection");
    assert_eq!(eligible("xberg-markdown-sceptre-ort"), 4);
}

fn build_provenance(
    entry: &benchmark_harness::bench_matrix::MatrixEntry,
    contract: &CohortContract,
    manifest_blake3: &str,
    fixture_provenance: &[FixtureProvenance],
) -> RunProvenance {
    use benchmark_harness::bench_matrix::ExecutionMode;

    let batch = matches!(entry.mode, ExecutionMode::Batch);
    let policy = benchmark_harness::adapter::declared_ocr_language_policy(&entry.framework);
    let eligible_languages: Vec<Option<String>> = contract
        .document_extensions
        .iter()
        .zip(contract.fixtures.iter())
        .filter(|(extension, fixture)| {
            supports_extension(&entry.framework, extension) && supports_fixture_language(&entry.framework, fixture)
        })
        .map(|(_, fixture)| {
            let descriptor: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(repo_path(&format!("fixtures/{fixture}"))).expect("read fixture descriptor"),
            )
            .expect("parse fixture descriptor");
            descriptor["metadata"]["ocr_language"].as_str().map(str::to_string)
        })
        .collect();
    let eligible_documents = eligible_languages.len();
    RunProvenance {
        schema_version: 2,
        harness_version: "test".to_string(),
        repository: RepositoryProvenance {
            commit: Some(SOURCE_SHA.to_string()),
            dirty: Some(false),
        },
        corpus: CorpusProvenance {
            cohort: Some(contract.manifest_name.to_string()),
            cohort_manifest_blake3: Some(manifest_blake3.to_string()),
            ordered_fixtures: fixture_provenance.to_vec(),
        },
        frameworks: vec![FrameworkProvenance {
            name: runtime_framework_name(entry),
            version: "0.0.0".to_string(),
            executable: None,
            models: Vec::new(),
            batch_capability: None,
            requested_workers: None,
            effective_workers: None,
            configured_thread_budget: None,
            worker_semantics: "test".to_string(),
            effective_warmup_iterations: 0,
            eligible_documents,
            batch_partitions: batch.then(|| policy.batch_partition_count(&eligible_languages, contract.batch_size)),
            ocr_language_policy: policy,
        }],
        timing: TimingProvenance {
            mode: entry.mode.benchmark_mode(),
            warmup_iterations: 0,
            benchmark_iterations: ITERATIONS,
            timeout_ms: 0,
            output_format: entry.output_format,
        },
        fixed_batch_size: batch.then_some(contract.batch_size),
    }
}

fn build_results(
    entry: &benchmark_harness::bench_matrix::MatrixEntry,
    contract: &CohortContract,
    cohort: Cohort,
    fixtures_root: &Path,
) -> Vec<BenchmarkResult> {
    contract
        .document_stems
        .iter()
        .zip(contract.document_extensions.iter())
        .zip(contract.fixtures.iter())
        .filter(|((_, extension), fixture)| {
            supports_extension(&entry.framework, extension) && supports_fixture_language(&entry.framework, fixture)
        })
        .map(|((stem, extension), fixture)| {
            let descriptor: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(fixtures_root.join(fixture)).expect("read fixture descriptor"),
            )
            .expect("parse fixture descriptor");
            let has_quality_ground_truth = descriptor["ground_truth"]["text_file"].is_string()
                || descriptor["ground_truth"]["markdown_file"].is_string();
            let has_structural_ground_truth = descriptor["ground_truth"]["markdown_file"].is_string();
            BenchmarkResult {
                framework: runtime_framework_name(entry),
                output_format: entry.output_format,
                file_path: PathBuf::from(format!("/workspace/test_documents/{stem}.{extension}")),
                file_size: 1,
                success: true,
                error_message: None,
                error_kind: ErrorKind::None,
                duration: Duration::from_millis(1),
                extraction_duration: None,
                subprocess_overhead: None,
                metrics: zero_metrics(),
                quality: has_quality_ground_truth.then(|| QualityMetrics {
                    f1_score_text: 0.9,
                    f1_score_numeric: 0.8,
                    f1_score_layout: (entry.output_format == OutputFormat::Markdown && has_structural_ground_truth)
                        .then_some(0.7),
                    quality_score: 0.85,
                    missing_tokens: vec![],
                    extra_tokens: vec![],
                    correct: false,
                }),
                iterations: (0..ITERATIONS)
                    .map(|index| IterationResult {
                        // mirror that here so the fixture matches real artifacts.
                        iteration: index + 1,
                        duration: Duration::from_millis(1),
                        extraction_duration: None,
                        metrics: zero_metrics(),
                    })
                    .collect(),
                statistics: None,
                cold_start_duration: None,
                file_extension: (*extension).to_string(),
                framework_capabilities: FrameworkCapabilities::default(),
                pdf_metadata: None,
                ocr_status: if cohort.expects_ocr() {
                    OcrStatus::Used
                } else {
                    OcrStatus::NotUsed
                },
                extracted_text: None,
                system_load: None,
            }
        })
        .collect()
}

/// A fully materialized, contract-conformant artifact tree ready to validate.
struct ArtifactScenario {
    _tree: FixtureTree,
    args: ValidateArtifactsArgs,
    contract: CohortContract,
}

fn artifact_scenario(cohort: Cohort) -> ArtifactScenario {
    let contract = cohort.contract();
    let tree = materialize_fixture_tree(cohort, &contract);
    let artifacts_dir = tree.cohort_manifest.parent().expect("root").join("artifacts");
    std::fs::create_dir_all(&artifacts_dir).expect("create artifacts dir");

    for entry in &contract.matrix {
        let run_dir = artifacts_dir.join(format!("{}-{RUN_ID}", entry.artifact)).join("run");
        let provenance = build_provenance(entry, &contract, &tree.manifest_blake3, &tree.fixture_provenance);
        write_run_provenance(&provenance, &run_dir.join("provenance.json")).expect("write provenance");
        let results = build_results(entry, &contract, cohort, &tree.fixtures_root);
        write_json(&results, &run_dir.join("results.json")).expect("write results");
    }

    let args = ValidateArtifactsArgs {
        cohort,
        aggregated_file: None,
        artifacts_dir: Some(artifacts_dir),
        cohort_manifest: Some(tree.cohort_manifest.clone()),
        fixtures_root: Some(tree.fixtures_root.clone()),
        source_sha: Some(SOURCE_SHA.to_string()),
        run_id: Some(RUN_ID.to_string()),
        iterations: ITERATIONS,
    };

    ArtifactScenario {
        _tree: tree,
        args,
        contract,
    }
}

fn provenance_path(scenario: &ArtifactScenario, matrix_index: usize) -> PathBuf {
    let entry = &scenario.contract.matrix[matrix_index];
    scenario
        .args
        .artifacts_dir
        .as_ref()
        .unwrap()
        .join(format!("{}-{RUN_ID}", entry.artifact))
        .join("run/provenance.json")
}

fn results_path(scenario: &ArtifactScenario, matrix_index: usize) -> PathBuf {
    let entry = &scenario.contract.matrix[matrix_index];
    scenario
        .args
        .artifacts_dir
        .as_ref()
        .unwrap()
        .join(format!("{}-{RUN_ID}", entry.artifact))
        .join("run/results.json")
}

fn tamper_json(path: &Path, mutate: impl FnOnce(&mut serde_json::Value)) {
    let mut value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    mutate(&mut value);
    std::fs::write(path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
}

fn assert_err_contains<T: std::fmt::Debug>(result: Result<T, Error>, needle: &str) {
    let error = result.expect_err("expected validation failure");
    let message = error.to_string();
    assert!(
        message.contains(needle),
        "expected error containing {needle:?}, got: {message}"
    );
}

fn required_count(contract: &CohortContract) -> usize {
    contract.matrix.iter().filter(|entry| !entry.optional).count()
}

/// Aggregate key of a required (non-optional) matrix entry. Validation skips optional groups, so a
/// tamper test must target a required group to observe the rejection deterministically — picking an
/// arbitrary group via HashMap iteration order is flaky, as it may land on a skipped optional group.
fn required_group_key(contract: &CohortContract) -> String {
    contract
        .matrix
        .iter()
        .find(|entry| !entry.optional)
        .expect("cohort has a required entry")
        .aggregate_key()
}

fn optional_artifact_dir(scenario: &ArtifactScenario) -> PathBuf {
    let entry = scenario
        .contract
        .matrix
        .iter()
        .find(|entry| entry.optional)
        .expect("cohort has an optional (best-effort) entry");
    scenario
        .args
        .artifacts_dir
        .as_ref()
        .unwrap()
        .join(format!("{}-{RUN_ID}", entry.artifact))
}

fn optional_matrix_index(contract: &CohortContract) -> usize {
    contract
        .matrix
        .iter()
        .position(|entry| entry.optional)
        .expect("cohort has an optional (best-effort) entry")
}

#[test]
fn accepts_exact_native_contract() {
    let scenario = artifact_scenario(Cohort::Native);
    let present = scenario.contract.matrix.len();
    let message = validate(&scenario.args).expect("native contract should validate");
    assert_eq!(message, format!("validated {present} native benchmark artifacts"));
}

#[test]
fn accepts_exact_ocr_contract() {
    let scenario = artifact_scenario(Cohort::Ocr);
    let present = scenario.contract.matrix.len();
    let message = validate(&scenario.args).expect("ocr contract should validate");
    assert_eq!(message, format!("validated {present} ocr benchmark artifacts"));
}

#[test]
fn accepts_exact_raw_contract_for_every_format_cohort() {
    // ~keep Every cohort exercises the same optional-present validation path, including family
    // cohorts where competitors intentionally cover only a supported subset of extensions.
    for cohort in Cohort::ALL {
        let scenario = artifact_scenario(cohort);
        validate(&scenario.args).unwrap_or_else(|error| panic!("{} raw contract failed: {error}", cohort.as_str()));
    }
}

#[test]
fn rejects_every_invalid_raw_quality_metric_with_path_and_index_context() {
    for (field, value) in [
        ("f1_score_text", serde_json::json!(-0.01)),
        ("f1_score_numeric", serde_json::json!(1.01)),
        ("f1_score_layout", serde_json::json!(1.01)),
        ("quality_score", serde_json::json!(2.0)),
    ] {
        let scenario = artifact_scenario(Cohort::Native);
        let path = results_path(&scenario, 0);
        tamper_json(&path, |results| {
            results[0]["quality"][field] = value;
        });

        let error = validate(&scenario.args).expect_err("invalid raw quality must fail");
        let message = error.to_string();
        let context = format!("{}: result 0: Benchmark error: Invalid result state", path.display());
        assert!(
            message.contains(&context) && message.contains(field),
            "expected path/index context and field {field:?}, got: {message}"
        );
    }
}

#[test]
fn rejects_missing_quality_only_when_the_fixture_has_quality_ground_truth() {
    let scenario = artifact_scenario(Cohort::Native);
    let path = results_path(&scenario, 0);
    tamper_json(&path, |results| {
        results[0]["quality"] = serde_json::Value::Null;
    });

    assert_err_contains(
        validate(&scenario.args),
        &format!("{}: result 0 quality presence mismatch", path.display()),
    );
}

#[test]
fn rejects_missing_markdown_sf1_only_when_the_fixture_has_structural_ground_truth() {
    let scenario = artifact_scenario(Cohort::Native);
    let path = results_path(&scenario, 0);
    let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let result_index = value
        .as_array()
        .unwrap()
        .iter()
        .position(|result| result["quality"]["f1_score_layout"].is_number())
        .expect("native markdown artifact has structural ground truth");
    tamper_json(&path, |results| {
        results[result_index]["quality"]["f1_score_layout"] = serde_json::Value::Null;
    });

    assert_err_contains(
        validate(&scenario.args),
        &format!("{}: result {result_index} SF1 presence mismatch", path.display()),
    );
}

#[test]
fn release_validation_rejects_plaintext_sf1_despite_generic_writer_compatibility() {
    let scenario = artifact_scenario(Cohort::Native);
    let matrix_index = scenario
        .contract
        .matrix
        .iter()
        .position(|entry| !entry.optional && entry.output_format == OutputFormat::Plaintext)
        .expect("native contract has required plaintext entry");
    let path = results_path(&scenario, matrix_index);
    tamper_json(&path, |results| {
        results[0]["quality"]["f1_score_layout"] = serde_json::json!(0.7);
    });

    assert_err_contains(
        validate(&scenario.args),
        &format!("{}: result 0 SF1 presence mismatch", path.display()),
    );
}

/// Index of a batch-mode xberg cell in the native matrix — the case that carries the runner's
/// `-batch` framework-name suffix.
fn batch_xberg_index(contract: &CohortContract) -> usize {
    use benchmark_harness::bench_matrix::ExecutionMode;
    contract
        .matrix
        .iter()
        .position(|entry| entry.framework.starts_with("xberg-") && matches!(entry.mode, ExecutionMode::Batch))
        .expect("native matrix has a batch xberg cell")
}

#[test]
fn accepts_batch_xberg_framework_with_mode_suffix() {
    // Batch xberg cells write `xberg-<fmt>-<pipeline>-batch`; the validator must accept that
    // suffixed name against the mode-independent matrix entry (mode is checked via timing.mode).
    let scenario = artifact_scenario(Cohort::Native);
    let index = batch_xberg_index(&scenario.contract);
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(provenance_path(&scenario, index)).unwrap()).unwrap();
    assert!(
        value["frameworks"][0]["name"].as_str().unwrap().ends_with("-batch"),
        "fixture must carry the runner's -batch suffix"
    );
    validate(&scenario.args).expect("suffixed batch xberg name must validate");
}

#[test]
fn rejects_batch_xberg_framework_with_wrong_base() {
    // Stripping the mode suffix must not mask a genuinely wrong base framework name.
    let scenario = artifact_scenario(Cohort::Native);
    let index = batch_xberg_index(&scenario.contract);
    tamper_json(&provenance_path(&scenario, index), |value| {
        value["frameworks"][0]["name"] = serde_json::json!("xberg-markdown-bogus-batch");
    });
    assert_err_contains(validate(&scenario.args), "framework mismatch");
}

#[test]
fn accepts_native_contract_when_optional_mineru_absent() {
    // A best-effort framework (MinerU) that never produced an artifact must not fail
    // validation: the baseline still publishes with the required frameworks.
    let scenario = artifact_scenario(Cohort::Native);
    std::fs::remove_dir_all(optional_artifact_dir(&scenario)).expect("remove optional artifact dir");
    let required = required_count(&scenario.contract);
    let message = validate(&scenario.args).expect("native contract should validate without mineru");
    assert_eq!(message, format!("validated {required} native benchmark artifacts"));
}

#[test]
fn should_reject_optional_raw_failure_without_diagnostic_when_present() {
    // ~keep Best-effort failures remain publishable only when their failure category and diagnostic
    // are complete; otherwise consumers cannot distinguish framework behavior from corrupt data.
    let scenario = artifact_scenario(Cohort::Native);
    let results = optional_artifact_dir(&scenario).join("run/results.json");
    tamper_json(&results, |value| {
        value[0]["success"] = serde_json::Value::Bool(false);
        value[0]["error_kind"] = serde_json::Value::String("timeout".to_string());
    });
    assert_err_contains(validate(&scenario.args), "success=false but error_message is None");
}

#[test]
fn should_accept_well_categorized_optional_raw_failure_when_present() {
    // ~keep Optional producer failures are useful comparison data when the artifact remains
    // structurally complete and carries an explicit category and diagnostic.
    let scenario = artifact_scenario(Cohort::Native);
    let results = optional_artifact_dir(&scenario).join("run/results.json");
    tamper_json(&results, |value| {
        value[0]["success"] = serde_json::Value::Bool(false);
        value[0]["error_kind"] = serde_json::Value::String("timeout".to_string());
        value[0]["error_message"] = serde_json::Value::String("timed out after 900 seconds".to_string());
    });
    validate(&scenario.args).expect("well-categorized optional failure should remain publishable");
}

#[test]
fn should_reject_optional_raw_infrastructure_failure_when_present() {
    // ~keep Best-effort applies only to framework-accountable failures; harness/config failures
    // invalidate the benchmark environment and must never flow into release data.
    let scenario = artifact_scenario(Cohort::Native);
    let results = optional_artifact_dir(&scenario).join("run/results.json");
    tamper_json(&results, |value| {
        value[0]["success"] = serde_json::Value::Bool(false);
        value[0]["error_kind"] = serde_json::Value::String("harness_error".to_string());
        value[0]["error_message"] = serde_json::Value::String("subprocess protocol failed".to_string());
    });
    assert_err_contains(validate(&scenario.args), "infrastructure error");
}

#[test]
fn should_reject_optional_raw_artifact_with_wrong_provenance_when_present() {
    // ~keep A present best-effort artifact has the same provenance trust boundary as every
    // required artifact; optionality must never bypass source-commit verification.
    let scenario = artifact_scenario(Cohort::Native);
    let index = optional_matrix_index(&scenario.contract);
    tamper_json(&provenance_path(&scenario, index), |value| {
        value["repository"]["commit"] = serde_json::Value::String("d".repeat(40));
    });
    assert_err_contains(validate(&scenario.args), "source SHA mismatch");
}

#[test]
fn should_reject_optional_raw_artifact_with_missing_fixture_when_present() {
    // ~keep Optional artifacts may be wholly absent, but a present artifact must cover the full
    // cohort; accepting a partial result would silently bias the published comparison.
    let scenario = artifact_scenario(Cohort::Native);
    let index = optional_matrix_index(&scenario.contract);
    tamper_json(&results_path(&scenario, index), |value| {
        value.as_array_mut().unwrap().pop();
    });
    assert_err_contains(validate(&scenario.args), "result fixture count mismatch");
}

#[test]
fn rejects_tampered_manifest_bytes() {
    let scenario = artifact_scenario(Cohort::Native);
    let mut manifest = std::fs::read(scenario.args.cohort_manifest.clone().unwrap()).unwrap();
    manifest.push(b' ');
    std::fs::write(scenario.args.cohort_manifest.as_ref().unwrap(), manifest).unwrap();
    assert_err_contains(validate(&scenario.args), "manifest BLAKE3 mismatch");
}

#[test]
fn rejects_wrong_fixture_digest_for_real_descriptor() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&provenance_path(&scenario, 0), |value| {
        value["corpus"]["ordered_fixtures"][0]["fixture_blake3"] = serde_json::Value::String("0".repeat(64));
    });
    assert_err_contains(validate(&scenario.args), "descriptor BLAKE3 mismatch");
}

#[test]
fn rejects_wrong_document_digest_for_real_document() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&provenance_path(&scenario, 0), |value| {
        value["corpus"]["ordered_fixtures"][0]["document_blake3"] = serde_json::Value::String("0".repeat(64));
    });
    assert_err_contains(validate(&scenario.args), "document BLAKE3 mismatch");
}

#[test]
fn rejects_wrong_document_bytes_for_real_document() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&provenance_path(&scenario, 0), |value| {
        let bytes = value["corpus"]["ordered_fixtures"][0]["document_bytes"]
            .as_u64()
            .unwrap();
        value["corpus"]["ordered_fixtures"][0]["document_bytes"] = serde_json::Value::from(bytes + 1);
    });
    assert_err_contains(validate(&scenario.args), "document size mismatch");
}

#[test]
fn rejects_unexpected_artifact() {
    let scenario = artifact_scenario(Cohort::Native);
    std::fs::create_dir(
        scenario
            .args
            .artifacts_dir
            .as_ref()
            .unwrap()
            .join("benchmarks-surprise-42"),
    )
    .unwrap();
    assert_err_contains(validate(&scenario.args), "unexpected");
}

#[test]
fn rejects_source_sha_mismatch() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&provenance_path(&scenario, 0), |value| {
        value["repository"]["commit"] = serde_json::Value::String("d".repeat(40));
    });
    assert_err_contains(validate(&scenario.args), "source SHA mismatch");
}

#[test]
fn rejects_timeout_result() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&results_path(&scenario, 0), |value| {
        value[0]["success"] = serde_json::Value::Bool(false);
        value[0]["error_kind"] = serde_json::Value::String("timeout".to_string());
        value[0]["error_message"] = serde_json::Value::String("timed out".to_string());
    });
    assert_err_contains(validate(&scenario.args), "failed");
}

#[test]
fn rejects_duplicate_fixture_result() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&results_path(&scenario, 0), |value| {
        let first_path = value[0]["file_path"].clone();
        value[1]["file_path"] = first_path;
    });
    assert_err_contains(validate(&scenario.args), "order/content mismatch");
}

#[test]
fn rejects_malformed_provenance() {
    let scenario = artifact_scenario(Cohort::Native);
    std::fs::write(provenance_path(&scenario, 0), "{").unwrap();
    assert_err_contains(validate(&scenario.args), "malformed");
}

#[test]
fn rejects_manifest_fixtures_when_not_an_array() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(scenario.args.cohort_manifest.as_ref().unwrap(), |value| {
        value["fixtures"] = serde_json::json!({});
    });
    assert!(validate(&scenario.args).is_err());
}

#[test]
fn rejects_framework_when_not_an_object() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&provenance_path(&scenario, 0), |value| {
        value["frameworks"] = serde_json::json!([null]);
    });
    assert!(validate(&scenario.args).is_err());
}

#[test]
fn rejects_result_row_when_not_an_object() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&results_path(&scenario, 0), |value| {
        value[0] = serde_json::Value::Null;
    });
    assert!(validate(&scenario.args).is_err());
}

#[test]
fn accepts_sequential_one_based_iterations() {
    // The runner numbers iterations 1-based ([1, 2, 3] for ITERATIONS = 3); the untampered
    // scenario fixture already reflects that, so this simply pins the happy path against
    // regressing back to a 0-based expectation.
    let scenario = artifact_scenario(Cohort::Native);
    validate(&scenario.args).expect("sequential 1-based iterations should validate");
}

#[test]
fn rejects_misordered_iterations() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&results_path(&scenario, 0), |value| {
        let second = value[0]["iterations"][1]["iteration"].clone();
        let third = value[0]["iterations"][2]["iteration"].clone();
        value[0]["iterations"][1]["iteration"] = third;
        value[0]["iterations"][2]["iteration"] = second;
    });
    assert_err_contains(validate(&scenario.args), "iteration order/duplicates mismatch");
}

#[test]
fn rejects_duplicate_iterations() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&results_path(&scenario, 0), |value| {
        let first = value[0]["iterations"][0]["iteration"].clone();
        value[0]["iterations"][1]["iteration"] = first;
    });
    assert_err_contains(validate(&scenario.args), "iteration order/duplicates mismatch");
}

#[test]
fn rejects_iteration_count_mismatch() {
    let scenario = artifact_scenario(Cohort::Native);
    tamper_json(&results_path(&scenario, 0), |value| {
        // the configured ITERATIONS.
        value[0]["iterations"].as_array_mut().unwrap().pop();
    });
    assert_err_contains(validate(&scenario.args), "iteration count mismatch");
}

fn build_aggregate_with(
    contract: &CohortContract,
    cohort: Cohort,
    include_entry: impl Fn(&benchmark_harness::bench_matrix::MatrixEntry) -> bool,
    mutate_results: impl FnOnce(&mut Vec<BenchmarkResult>),
) -> NewConsolidatedResults {
    let fixture_provenance: Vec<FixtureProvenance> = contract
        .fixtures
        .iter()
        .map(|fixture| FixtureProvenance {
            fixture: (*fixture).to_string(),
            fixture_blake3: "b".repeat(64),
            document_blake3: "c".repeat(64),
            document_bytes: 1,
        })
        .collect();
    let run_provenance: Vec<RunProvenanceRecord> = contract
        .matrix
        .iter()
        .filter(|entry| include_entry(entry))
        .map(|entry| RunProvenanceRecord {
            source_dir: entry.artifact.clone(),
            provenance: Some(build_provenance(
                entry,
                contract,
                contract.manifest_blake3,
                &fixture_provenance,
            )),
            missing_reason: None,
        })
        .collect();
    // ~keep Consolidation tags every framework loaded from a batch artifact directory. This
    // builder bypasses that filesystem loader, so mirror the tag before deriving aggregate keys.
    let results: Vec<BenchmarkResult> = contract
        .matrix
        .iter()
        .filter(|entry| include_entry(entry))
        .flat_map(|entry| {
            let mut entry_results = build_results(entry, contract, cohort, &repo_path("fixtures"));
            let framework = aggregate_framework_name(entry);
            for result in &mut entry_results {
                result.framework.clone_from(&framework);
            }
            entry_results
        })
        .collect();
    let mut results = results;
    mutate_results(&mut results);
    let mut aggregate = benchmark_harness::aggregate_new_format(&results);
    aggregate.run_provenance = run_provenance;
    benchmark_harness::aggregate::apply_pinned_cohort_comparison(&mut aggregate).expect("apply cohort comparison");
    aggregate
}

fn build_aggregate(contract: &CohortContract, cohort: Cohort) -> NewConsolidatedResults {
    build_aggregate_with(contract, cohort, |_| true, |_| {})
}

fn write_aggregate(aggregate: &NewConsolidatedResults) -> (TempDir, PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("aggregated.json");
    std::fs::write(&path, serde_json::to_string_pretty(aggregate).unwrap()).unwrap();
    (root, path)
}

fn write_json_value(value: &serde_json::Value) -> (TempDir, PathBuf) {
    let root = tempfile::tempdir().expect("tempdir");
    let path = root.path().join("aggregated.json");
    std::fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    (root, path)
}

fn aggregate_args(cohort: Cohort, aggregated_file: PathBuf) -> ValidateArtifactsArgs {
    ValidateArtifactsArgs {
        cohort,
        aggregated_file: Some(aggregated_file),
        artifacts_dir: None,
        cohort_manifest: None,
        fixtures_root: None,
        source_sha: None,
        run_id: None,
        iterations: ITERATIONS,
    }
}

#[test]
fn accepts_exact_native_aggregate_contract() {
    let contract = Cohort::Native.contract();
    let aggregate = build_aggregate(&contract, Cohort::Native);
    let (_root, path) = write_aggregate(&aggregate);
    let present = contract.matrix.len();
    let message = validate(&aggregate_args(Cohort::Native, path)).expect("native aggregate should validate");
    assert_eq!(
        message,
        format!(
            "validated {present} native aggregate keys and {} fixture rows",
            present * contract.fixtures.len()
        )
    );
}

#[test]
fn accepts_exact_ocr_aggregate_contract() {
    let contract = Cohort::Ocr.contract();
    let aggregate = build_aggregate(&contract, Cohort::Ocr);
    let (_root, path) = write_aggregate(&aggregate);
    let present = contract.matrix.len();
    let expected_rows: usize = contract
        .matrix
        .iter()
        .map(|entry| {
            contract
                .document_extensions
                .iter()
                .zip(contract.fixtures.iter())
                .filter(|(extension, fixture)| {
                    supports_extension(&entry.framework, extension)
                        && supports_fixture_language(&entry.framework, fixture)
                })
                .count()
        })
        .sum();
    let message = validate(&aggregate_args(Cohort::Ocr, path)).expect("ocr aggregate should validate");
    assert_eq!(
        message,
        format!("validated {present} ocr aggregate keys and {expected_rows} fixture rows")
    );
}

#[test]
fn rejects_aggregate_row_when_nested_quality_is_removed_for_a_ground_truth_fixture() {
    let contract = Cohort::Native.contract();
    let aggregate = build_aggregate(&contract, Cohort::Native);
    let mut value = serde_json::to_value(&aggregate).unwrap();
    let row = value["per_fixture_results"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|row| row["success"] == true && row["quality"].is_object())
        .expect("aggregate contains a successful quality row");
    let fixture_id = row["fixture_id"].as_str().unwrap().to_string();
    row["quality"] = serde_json::Value::Null;
    let (_root, path) = write_json_value(&value);

    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        &format!("fixture row {fixture_id} quality presence mismatch"),
    );
}

#[test]
fn rejects_aggregate_row_when_tf1_or_sf1_scalar_projection_is_removed() {
    for field in ["f1_text", "f1_layout"] {
        let contract = Cohort::Native.contract();
        let aggregate = build_aggregate(&contract, Cohort::Native);
        let mut value = serde_json::to_value(&aggregate).unwrap();
        let row = value["per_fixture_results"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row["success"] == true && row[field].is_number())
            .unwrap_or_else(|| panic!("aggregate contains numeric {field}"));
        row[field] = serde_json::Value::Null;
        let (_root, path) = write_json_value(&value);

        assert_err_contains(
            validate(&aggregate_args(Cohort::Native, path)),
            "fixture-row quality projection mismatch",
        );
    }
}

#[test]
fn accepts_exact_aggregate_contract_for_every_format_cohort() {
    // ~keep This guards supported-subset cardinality and format-support validation across all
    // rendered-page and heterogeneous format families, not only the PDF release cohorts.
    for cohort in Cohort::ALL {
        let contract = cohort.contract();
        let aggregate = build_aggregate(&contract, cohort);
        let (_root, path) = write_aggregate(&aggregate);
        validate(&aggregate_args(cohort, path))
            .unwrap_or_else(|error| panic!("{} aggregate contract failed: {error}", cohort.as_str()));
    }
}

#[test]
fn rejects_fabricated_aggregate_comparison() {
    let contract = Cohort::Native.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    aggregate.comparison.throughput_ranking.push(RankedFramework {
        framework_mode: "fabricated:markdown:single".to_string(),
        rank: 1,
        value: 999.0,
        relative: 1.0,
        optional: false,
    });
    let (_root, path) = write_aggregate(&aggregate);

    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "comparison rankings mismatch",
    );
}

#[test]
fn rejects_missing_aggregate_provenance() {
    let contract = Cohort::Native.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    aggregate.run_provenance.clear();
    let (_root, path) = write_aggregate(&aggregate);

    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "aggregate provenance count mismatch",
    );
}

#[test]
fn rejects_fabricated_aggregate_provenance_settings() {
    let contract = Cohort::Native.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    let batch = aggregate
        .run_provenance
        .iter_mut()
        .find(|record| {
            record
                .provenance
                .as_ref()
                .is_some_and(|provenance| provenance.fixed_batch_size.is_some())
        })
        .expect("batch provenance");
    batch.provenance.as_mut().unwrap().fixed_batch_size = None;
    let (_root, path) = write_aggregate(&aggregate);

    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "aggregate provenance settings mismatch",
    );
}

#[test]
fn rejects_fabricated_aggregate_metadata_counts() {
    let contract = Cohort::Native.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    aggregate.metadata.total_results += 1;
    let (_root, path) = write_aggregate(&aggregate);

    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "consolidation metadata mismatch",
    );
}

#[test]
fn rejects_aggregate_metrics_fabricated_independently_of_fixture_rows() {
    let contract = Cohort::Native.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    let group = aggregate
        .by_framework_mode
        .values_mut()
        .next()
        .expect("aggregate group");
    group.overall_performance.as_mut().unwrap().throughput.p50 = 999.0;
    let (_root, path) = write_aggregate(&aggregate);

    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "aggregate metrics mismatch",
    );
}

#[test]
fn accepts_native_aggregate_when_optional_mineru_absent() {
    // validation must still pass on the required frameworks.
    let contract = Cohort::Native.contract();
    // ~keep Filter before aggregation so every production-derived view consistently excludes the
    // absent best-effort framework, including failure and format-support summaries.
    let aggregate = build_aggregate_with(&contract, Cohort::Native, |entry| !entry.optional, |_| {});

    let (_root, path) = write_aggregate(&aggregate);
    let required = required_count(&contract);
    let message =
        validate(&aggregate_args(Cohort::Native, path)).expect("native aggregate should validate without mineru");
    assert_eq!(
        message,
        format!(
            "validated {required} native aggregate keys and {} fixture rows",
            required * contract.fixtures.len()
        )
    );
}

#[test]
fn should_reject_invalid_optional_aggregate_group_when_present() {
    // ~keep Absence is the only relaxation for an optional aggregate group. A present group is
    // release data and must retain the same file-type and sample-count integrity checks.
    let contract = Cohort::Native.contract();
    let optional_key = contract
        .matrix
        .iter()
        .find(|entry| entry.optional)
        .expect("native cohort has an optional entry")
        .aggregate_key();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    aggregate
        .by_framework_mode
        .get_mut(&optional_key)
        .expect("aggregate contains optional group")
        .by_file_type
        .clear();
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "has no file-type metrics",
    );
}

#[test]
fn should_reject_failed_optional_aggregate_row_when_present() {
    // ~keep Present optional rows must be validated rather than filtered out; otherwise failed
    // executions can be published even when the optional group itself appears structurally valid.
    let contract = Cohort::Native.contract();
    let optional_framework = contract
        .matrix
        .iter()
        .find(|entry| entry.optional)
        .expect("native cohort has an optional entry")
        .framework
        .clone();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    aggregate
        .per_fixture_results
        .iter_mut()
        .find(|row| row.framework == optional_framework)
        .expect("aggregate contains optional row")
        .success = false;
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(validate(&aggregate_args(Cohort::Native, path)), "failed fixture rows");
}

#[test]
fn should_accept_well_categorized_optional_aggregate_failure_when_present() {
    // ~keep A best-effort framework may publish partial failures when group accounting and row
    // diagnostics agree, preserving useful measurements without weakening structural validation.
    let contract = Cohort::Native.contract();
    let optional_entry = contract
        .matrix
        .iter()
        .find(|entry| entry.optional)
        .expect("native cohort has an optional entry");
    let optional_runtime_framework = aggregate_framework_name(optional_entry);
    // ~keep Inject the failure before production aggregation so bucket counters, rows, rankings,
    // and failure summaries are derived from one source of truth.
    let aggregate = build_aggregate_with(
        &contract,
        Cohort::Native,
        |_| true,
        |results| {
            let result = results
                .iter_mut()
                .find(|result| result.framework == optional_runtime_framework)
                .expect("aggregate input contains optional result");
            result.success = false;
            result.error_kind = ErrorKind::Timeout;
            result.error_message = Some("timed out after 900 seconds".to_string());
        },
    );
    let failed_row = aggregate
        .per_fixture_results
        .iter()
        .find(|row| row.framework == optional_entry.framework && !row.success)
        .expect("aggregate retains optional failure row");
    assert_eq!(failed_row.error_kind.as_deref(), Some("Timeout"));
    let timeout_count = aggregate.by_framework_mode[&optional_entry.aggregate_key()].by_file_type["pdf"]
        .no_ocr
        .as_ref()
        .expect("native group uses no_ocr")
        .timeouts;
    assert_eq!(timeout_count, 1);
    let (_root, path) = write_aggregate(&aggregate);
    validate(&aggregate_args(Cohort::Native, path)).expect("categorized optional aggregate failure should validate");
}

#[test]
fn should_reject_optional_aggregate_infrastructure_failure_when_present() {
    // ~keep Optional aggregate groups cannot convert runner infrastructure faults into
    // best-effort framework data, even when their total cardinality remains internally consistent.
    let contract = Cohort::Native.contract();
    let optional_entry = contract
        .matrix
        .iter()
        .find(|entry| entry.optional)
        .expect("native cohort has an optional entry");
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    let bucket = aggregate
        .by_framework_mode
        .get_mut(&optional_entry.aggregate_key())
        .expect("aggregate contains optional group")
        .by_file_type
        .get_mut("pdf")
        .expect("native group contains pdf")
        .no_ocr
        .as_mut()
        .expect("native group uses no_ocr");
    bucket.successful_sample_count -= 1;
    bucket.harness_errors = 1;
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "infrastructure failures",
    );
}

#[test]
fn should_reject_optional_aggregate_row_with_infrastructure_error_kind() {
    // ~keep Row diagnostics must agree with the accountable bucket categories; a valid total alone
    // cannot disguise a harness fault as a framework-level best-effort failure.
    let contract = Cohort::Native.contract();
    let optional_entry = contract
        .matrix
        .iter()
        .find(|entry| entry.optional)
        .expect("native cohort has an optional entry");
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    let bucket = aggregate
        .by_framework_mode
        .get_mut(&optional_entry.aggregate_key())
        .expect("aggregate contains optional group")
        .by_file_type
        .get_mut("pdf")
        .expect("native group contains pdf")
        .no_ocr
        .as_mut()
        .expect("native group uses no_ocr");
    bucket.successful_sample_count -= 1;
    bucket.timeouts = 1;
    let row = aggregate
        .per_fixture_results
        .iter_mut()
        .find(|row| row.framework == optional_entry.framework)
        .expect("aggregate contains optional row");
    row.success = false;
    row.error_kind = Some("HarnessError".to_string());
    row.error_message = Some("subprocess protocol failed".to_string());
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "accountable error kind",
    );
}

#[test]
fn should_reject_invalid_optional_framework_format_support_when_present() {
    // ~keep The format-support matrix is a published explanation for absent framework/format
    // pairs. Every present entry, including an optional framework, must reference a cohort format.
    let contract = Cohort::Native.contract();
    let optional_framework = contract
        .matrix
        .iter()
        .find(|entry| entry.optional)
        .expect("native cohort has an optional entry")
        .framework
        .clone();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    aggregate.format_support.file_types = vec!["pdf".to_string()];
    aggregate
        .format_support
        .unsupported
        .insert(optional_framework, vec!["exe".to_string()]);
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(validate(&aggregate_args(Cohort::Native, path)), "format support");
}

#[test]
fn rejects_unexpected_aggregate_key() {
    let contract = Cohort::Native.contract();
    let aggregate = build_aggregate(&contract, Cohort::Native);
    let mut value = serde_json::to_value(&aggregate).unwrap();
    let map = value["by_framework_mode"].as_object_mut().unwrap();
    let (first_key, first_value) = map
        .iter()
        .next()
        .map(|(key, value)| (key.clone(), value.clone()))
        .unwrap();
    map.remove(&first_key);
    map.insert("surprise:markdown:single".to_string(), first_value);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("aggregated.json");
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    assert_err_contains(validate(&aggregate_args(Cohort::Native, path)), "unexpected");
}

#[test]
fn rejects_native_aggregate_with_only_with_ocr_bucket() {
    let contract = Cohort::Native.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    for group in aggregate.by_framework_mode.values_mut() {
        let file_group = group.by_file_type.get_mut("pdf").unwrap();
        file_group.with_ocr = file_group.no_ocr.take();
    }
    let (_root, path) = write_aggregate(&aggregate);
    assert!(validate(&aggregate_args(Cohort::Native, path)).is_err());
}

#[test]
fn rejects_ocr_aggregate_with_only_no_ocr_bucket() {
    let contract = Cohort::Ocr.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Ocr);
    for group in aggregate.by_framework_mode.values_mut() {
        let file_group = group.by_file_type.get_mut("pdf").unwrap();
        file_group.no_ocr = file_group.with_ocr.take();
    }
    let (_root, path) = write_aggregate(&aggregate);
    assert!(validate(&aggregate_args(Cohort::Ocr, path)).is_err());
}

#[test]
fn rejects_aggregate_group_without_file_type_metrics() {
    // A group with no file-type buckets can never account for the cohort's fixtures.
    let contract = Cohort::Native.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Native);
    let key = required_group_key(&contract);
    let first_group = aggregate.by_framework_mode.get_mut(&key).unwrap();
    first_group.by_file_type.clear();
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(
        validate(&aggregate_args(Cohort::Native, path)),
        "has no file-type metrics",
    );
}

#[test]
fn accepts_aggregate_with_multiple_file_type_buckets() {
    // The office cohort spans 7 extensions (docx×2, doc, pptx, ppt, xlsx, odt, rtf); build_aggregate
    // now emits that real per-extension bucket shape, which must validate.
    let contract = Cohort::Office.contract();
    let aggregate = build_aggregate(&contract, Cohort::Office);
    let (_root, path) = write_aggregate(&aggregate);
    validate(&aggregate_args(Cohort::Office, path)).expect("multi-file-type office aggregate should validate");
}

#[test]
fn rejects_aggregate_with_unexpected_file_type_bucket() {
    // A file-type bucket the cohort's fixtures don't contain must be rejected, even if counts sum.
    let contract = Cohort::Office.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Office);
    let key = required_group_key(&contract);
    let group = aggregate.by_framework_mode.get_mut(&key).unwrap();
    // Move all `docx` samples (2) into a bogus `pdf` bucket: totals still sum to 8, but pdf is not
    // an office extension and docx is now missing.
    let docx = group.by_file_type.remove("docx").unwrap();
    group.by_file_type.insert(
        "pdf".to_string(),
        FileTypeAggregation {
            file_type: "pdf".to_string(),
            ..docx
        },
    );
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(validate(&aggregate_args(Cohort::Office, path)), "file-type buckets");
}

#[test]
fn rejects_aggregate_with_wrong_file_type_sample_count() {
    // The right extensions but a mis-bucketed count (docx should be 2, not 1) must be rejected —
    // the per-extension check the sum-only check used to miss.
    let contract = Cohort::Office.contract();
    let mut aggregate = build_aggregate(&contract, Cohort::Office);
    let key = required_group_key(&contract);
    let group = aggregate.by_framework_mode.get_mut(&key).unwrap();
    group
        .by_file_type
        .get_mut("docx")
        .unwrap()
        .no_ocr
        .as_mut()
        .unwrap()
        .total_sample_count = 1;
    let (_root, path) = write_aggregate(&aggregate);
    assert_err_contains(
        validate(&aggregate_args(Cohort::Office, path)),
        "success/error counts do not match total_sample_count",
    );
}

#[test]
fn rejects_aggregate_row_when_not_an_object() {
    let contract = Cohort::Native.contract();
    let aggregate = build_aggregate(&contract, Cohort::Native);
    let mut value = serde_json::to_value(&aggregate).unwrap();
    value["per_fixture_results"][0] = serde_json::Value::Null;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("aggregated.json");
    std::fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    assert!(validate(&aggregate_args(Cohort::Native, path)).is_err());
}
