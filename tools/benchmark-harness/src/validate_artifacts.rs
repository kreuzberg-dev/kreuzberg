//! Validate the exact benchmark artifact contract for one fixed cohort.
//!
//! Ported from `scripts/ci/benchmarks/validate-benchmark-artifacts.py`. Two modes are
//! supported, matching the Python CLI:
//!
//! - **artifact mode** (`aggregated_file` is `None`): validates one directory of raw
//!   per-framework `run/{provenance,results}.json` artifacts against [`crate::bench_matrix`]'s
//!   pinned cohort contract.
//! - **aggregate mode** (`aggregated_file` is `Some`): validates one consolidated
//!   `aggregated.json` (as produced by the `consolidate` subcommand) against the same contract.

use std::collections::{HashMap, HashSet};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::adapter::declared_ocr_language_policy;
use crate::aggregate::{
    NewConsolidatedResults, PerFixtureRow, PerformancePercentiles, SCHEMA_VERSION, comparison_for_cohort,
    extract_framework_and_mode,
};
use crate::bench_matrix::{Cohort, CohortContract, ExecutionMode, MatrixEntry};
use crate::fixture::Fixture;
use crate::provenance::RunProvenance;
use crate::types::{BenchmarkResult, ErrorKind, OcrStatus, OutputFormat, PerformanceMetrics, QualityMetrics};
use crate::{Error, Result};

/// Provenance schema version every `provenance.json` must record.
///
/// Mirrors the private `PROVENANCE_SCHEMA_VERSION` constant in [`crate::provenance`]; kept as a
/// named constant here (rather than a literal `2`) because that constant is not exported.
const EXPECTED_PROVENANCE_SCHEMA_VERSION: u32 = 2;

/// Inputs for [`validate`], mirroring the Python script's `argparse` surface.
#[derive(Debug, Clone)]
pub struct ValidateArtifactsArgs {
    /// Which cohort's release contract to validate against.
    pub cohort: Cohort,
    /// Path to a consolidated `aggregated.json`. When set, aggregate mode runs. `fixtures_root`
    /// may override the checked-in fixture directory used for quality-contract metadata.
    pub aggregated_file: Option<PathBuf>,
    /// Directory containing one subdirectory per expected artifact (artifact mode only).
    pub artifacts_dir: Option<PathBuf>,
    /// Path to the pinned cohort manifest JSON (artifact mode only).
    pub cohort_manifest: Option<PathBuf>,
    /// Root directory fixture paths in the manifest are resolved against (artifact mode only).
    pub fixtures_root: Option<PathBuf>,
    /// Benchmark source revision every `provenance.json` must record (artifact mode only).
    pub source_sha: Option<String>,
    /// Run identifier suffix shared by every expected artifact directory (artifact mode only).
    pub run_id: Option<String>,
    /// Benchmark iterations every `provenance.json`/`results.json` must record.
    pub iterations: usize,
}

/// Validate one cohort's benchmark artifact contract, dispatching to artifact or aggregate mode.
///
/// Returns a human-readable summary line on success, matching the Python script's stdout.
pub fn validate(args: &ValidateArtifactsArgs) -> Result<String> {
    let contract = args.cohort.contract();
    match &args.aggregated_file {
        Some(aggregated_file) => {
            let default_fixtures_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
            let fixtures_root = args.fixtures_root.as_deref().unwrap_or(&default_fixtures_root);
            let quality_expectations = fixture_quality_expectations(fixtures_root, &contract)?;
            validate_aggregate(
                aggregated_file,
                args.cohort,
                &contract,
                args.iterations,
                &quality_expectations,
            )
        }
        None => {
            let (artifacts_dir, cohort_manifest, fixtures_root, source_sha, run_id) = require_artifact_args(args)?;
            validate_raw_artifacts(
                artifacts_dir,
                cohort_manifest,
                fixtures_root,
                source_sha,
                run_id,
                args.iterations,
                args.cohort,
                &contract,
            )
        }
    }
}

fn require_artifact_args(args: &ValidateArtifactsArgs) -> Result<(&Path, &Path, &Path, &str, &str)> {
    let mut missing = Vec::new();
    if args.artifacts_dir.is_none() {
        missing.push("artifacts-dir");
    }
    if args.cohort_manifest.is_none() {
        missing.push("cohort-manifest");
    }
    if args.fixtures_root.is_none() {
        missing.push("fixtures-root");
    }
    if args.source_sha.as_deref().unwrap_or_default().is_empty() {
        missing.push("source-sha");
    }
    if args.run_id.as_deref().unwrap_or_default().is_empty() {
        missing.push("run-id");
    }
    if !missing.is_empty() {
        return Err(Error::Config(format!(
            "artifact validation requires: {}",
            missing.join(", ")
        )));
    }
    Ok((
        args.artifacts_dir.as_deref().expect("checked above"),
        args.cohort_manifest.as_deref().expect("checked above"),
        args.fixtures_root.as_deref().expect("checked above"),
        args.source_sha.as_deref().expect("checked above"),
        args.run_id.as_deref().expect("checked above"),
    ))
}

fn require(condition: bool, message: impl Into<String>) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(Error::Benchmark(message.into()))
    }
}

fn contract_error(message: impl Into<String>) -> Error {
    Error::Benchmark(message.into())
}

/// One fixture's expected identity, derived from the real fixture descriptor and document bytes
/// referenced by the pinned cohort contract.
#[derive(Debug)]
struct ExpectedFixture {
    fixture: String,
    fixture_blake3: String,
    document_blake3: String,
    document_bytes: u64,
    document_name: String,
    has_quality_ground_truth: bool,
    has_structural_ground_truth: bool,
    ocr_language: Option<String>,
}

#[derive(Debug, Clone)]
struct FixtureQualityExpectation {
    has_quality_ground_truth: bool,
    has_structural_ground_truth: bool,
    ocr_language: Option<String>,
}

fn fixture_quality_expectations(
    fixtures_root: &Path,
    contract: &CohortContract,
) -> Result<Vec<FixtureQualityExpectation>> {
    contract
        .fixtures
        .iter()
        .map(|fixture| {
            let descriptor: Fixture = load_typed_json(&fixtures_root.join(fixture))?;
            Ok(FixtureQualityExpectation {
                has_quality_ground_truth: descriptor.ground_truth.as_ref().is_some_and(|ground_truth| {
                    ground_truth.text_file.is_some() || ground_truth.markdown_file.is_some()
                }),
                has_structural_ground_truth: descriptor
                    .ground_truth
                    .as_ref()
                    .is_some_and(|ground_truth| ground_truth.markdown_file.is_some()),
                ocr_language: descriptor.ocr_language().map(str::to_string),
            })
        })
        .collect()
}

/// Compute a BLAKE3 digest identical to the `b3sum` CLI output the Python script shelled out to.
fn blake3_file(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Err(contract_error(format!("{}: expected a regular file", path.display())));
    }
    let mut file = std::fs::File::open(path).map_err(Error::Io)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(Error::Io)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn load_json_text(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|error| contract_error(format!("{}: malformed or unreadable JSON: {error}", path.display())))
}

fn load_json_value(path: &Path) -> Result<serde_json::Value> {
    let text = load_json_text(path)?;
    serde_json::from_str(&text)
        .map_err(|error| contract_error(format!("{}: malformed or unreadable JSON: {error}", path.display())))
}

fn load_typed_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = load_json_text(path)?;
    serde_json::from_str(&text)
        .map_err(|error| contract_error(format!("{}: malformed or unreadable JSON: {error}", path.display())))
}

/// Basename of a `/`- or `\`-separated path string, matching `PurePosixPath(...).name`.
fn posix_basename(raw: &str) -> String {
    raw.replace('\\', "/").rsplit('/').next().unwrap_or(raw).to_string()
}

fn describe_set_mismatch<'a>(
    label: &str,
    expected: impl Iterator<Item = &'a str>,
    actual: impl Iterator<Item = &'a str>,
) -> String {
    let expected: HashSet<&str> = expected.collect();
    let actual: HashSet<&str> = actual.collect();
    let mut missing: Vec<&str> = expected.difference(&actual).copied().collect();
    missing.sort_unstable();
    let mut unexpected: Vec<&str> = actual.difference(&expected).copied().collect();
    unexpected.sort_unstable();
    format!("{label} mismatch; missing={missing:?}, unexpected={unexpected:?}")
}

fn validate_manifest(path: &Path, contract: &CohortContract) -> Result<String> {
    let manifest = load_json_value(path)?;
    let object = manifest
        .as_object()
        .ok_or_else(|| contract_error(format!("{}: manifest must be an object", path.display())))?;
    require(
        object.get("schema_version").and_then(serde_json::Value::as_u64) == Some(1),
        format!("{}: unexpected schema_version", path.display()),
    )?;
    require(
        object.get("name").and_then(serde_json::Value::as_str) == Some(contract.manifest_name),
        format!("{}: unexpected cohort name", path.display()),
    )?;
    require(
        object.get("batch_size").and_then(serde_json::Value::as_u64) == Some(contract.batch_size as u64),
        format!("{}: unexpected batch_size", path.display()),
    )?;
    let fixtures = object
        .get("fixtures")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| contract_error(format!("{}: fixtures must be an array", path.display())))?;
    let fixtures_match = fixtures
        .iter()
        .map(serde_json::Value::as_str)
        .collect::<Option<Vec<&str>>>()
        .is_some_and(|values| values == contract.fixtures);
    require(
        fixtures_match,
        format!("{}: fixture order/content mismatch", path.display()),
    )?;
    let digest = blake3_file(path)?;
    require(
        digest == contract.manifest_blake3,
        format!("{}: cohort manifest BLAKE3 mismatch", path.display()),
    )?;
    Ok(digest)
}

fn expected_fixtures(fixtures_root: &Path, contract: &CohortContract) -> Result<Vec<ExpectedFixture>> {
    let mut expected = Vec::with_capacity(contract.fixtures.len());
    for fixture in contract.fixtures {
        let descriptor_path = fixtures_root.join(fixture);
        let descriptor = Fixture::from_file(&descriptor_path)?;
        require(
            !descriptor.document.as_os_str().is_empty(),
            format!("{}: document must be a non-empty string", descriptor_path.display()),
        )?;
        let document_path = descriptor.validated_document_path(&descriptor_path)?;
        let document_name = descriptor
            .document
            .to_str()
            .map(posix_basename)
            .ok_or_else(|| contract_error(format!("{}: document path is not UTF-8", descriptor_path.display())))?;
        let has_quality_ground_truth = descriptor
            .ground_truth
            .as_ref()
            .is_some_and(|ground_truth| ground_truth.text_file.is_some() || ground_truth.markdown_file.is_some());
        let has_structural_ground_truth = descriptor
            .ground_truth
            .as_ref()
            .is_some_and(|ground_truth| ground_truth.markdown_file.is_some());
        expected.push(ExpectedFixture {
            fixture: (*fixture).to_string(),
            fixture_blake3: blake3_file(&descriptor_path)?,
            document_blake3: blake3_file(&document_path)?,
            document_bytes: std::fs::metadata(&document_path).map_err(Error::Io)?.len(),
            document_name,
            has_quality_ground_truth,
            has_structural_ground_truth,
            ocr_language: descriptor.ocr_language().map(str::to_string),
        });
    }

    let names: Vec<&str> = expected.iter().map(|item| item.document_name.as_str()).collect();
    let unique_names: HashSet<&str> = names.iter().copied().collect();
    require(
        unique_names.len() == names.len(),
        "cohort document basenames must be unique",
    )?;

    let stems: Vec<&str> = names
        .iter()
        .map(|name| {
            Path::new(name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(name)
        })
        .collect();
    require(
        stems == contract.document_stems,
        "cohort document identities do not match the release contract",
    )?;

    let extensions: Vec<String> = names
        .iter()
        .map(|name| {
            Path::new(name)
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase()
        })
        .collect();
    require(
        extensions
            .iter()
            .map(String::as_str)
            .eq(contract.document_extensions.iter().copied()),
        "cohort document extensions do not match the release contract",
    )?;

    Ok(expected)
}

fn read_artifact_dirs(root: &Path) -> Result<std::collections::HashMap<String, PathBuf>> {
    let mut dirs = std::collections::HashMap::new();
    for entry in std::fs::read_dir(root).map_err(Error::Io)? {
        let entry = entry.map_err(Error::Io)?;
        let path = entry.path();
        if path.is_dir()
            && let Some(name) = path.file_name().and_then(|name| name.to_str())
        {
            dirs.insert(name.to_string(), path);
        }
    }
    Ok(dirs)
}

fn only_file(root: &Path, filename: &str) -> Result<PathBuf> {
    let mut matches = Vec::new();
    collect_matching_files(root, filename, &mut matches)?;
    matches.sort();
    if matches.len() != 1 {
        return Err(contract_error(format!(
            "{}: expected exactly one {filename}, found {}",
            root.display(),
            matches.len()
        )));
    }
    Ok(matches.remove(0))
}

fn collect_matching_files(dir: &Path, filename: &str, matches: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir).map_err(Error::Io)? {
        let entry = entry.map_err(Error::Io)?;
        let path = entry.path();
        if path.is_dir() {
            collect_matching_files(&path, filename, matches)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some(filename) {
            matches.push(path);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn validate_provenance(
    provenance: &RunProvenance,
    path: &Path,
    entry: &MatrixEntry,
    contract: &CohortContract,
    manifest_blake3: &str,
    expected: &[ExpectedFixture],
    source_sha: &str,
    iterations: usize,
    eligible_documents: usize,
    expected_partitions: Option<usize>,
) -> Result<()> {
    require(
        provenance.schema_version == EXPECTED_PROVENANCE_SCHEMA_VERSION,
        format!("{}: unexpected provenance schema", path.display()),
    )?;
    require(
        provenance.repository.commit.as_deref() == Some(source_sha),
        format!("{}: source SHA mismatch", path.display()),
    )?;
    require(
        provenance.repository.dirty == Some(false),
        format!("{}: benchmark checkout was dirty", path.display()),
    )?;

    require(
        provenance.corpus.cohort.as_deref() == Some(contract.manifest_name),
        format!("{}: cohort name mismatch", path.display()),
    )?;
    require(
        provenance.corpus.cohort_manifest_blake3.as_deref() == Some(manifest_blake3),
        format!("{}: cohort manifest hash mismatch", path.display()),
    )?;

    require(
        provenance.corpus.ordered_fixtures.len() == expected.len(),
        format!("{}: fixture count mismatch", path.display()),
    )?;
    for (index, (item, expected_item)) in provenance.corpus.ordered_fixtures.iter().zip(expected).enumerate() {
        require(
            item.fixture == expected_item.fixture,
            format!("{}: fixture {index} identity/order mismatch", path.display()),
        )?;
        require(
            item.fixture_blake3 == expected_item.fixture_blake3,
            format!("{}: fixture {index} descriptor BLAKE3 mismatch", path.display()),
        )?;
        require(
            item.document_blake3 == expected_item.document_blake3,
            format!("{}: fixture {index} document BLAKE3 mismatch", path.display()),
        )?;
        require(
            item.document_bytes == expected_item.document_bytes,
            format!("{}: fixture {index} document size mismatch", path.display()),
        )?;
    }

    require(
        provenance.timing.mode == entry.mode.benchmark_mode(),
        format!("{}: execution mode mismatch", path.display()),
    )?;
    require(
        provenance.timing.benchmark_iterations == iterations,
        format!("{}: iteration count mismatch", path.display()),
    )?;
    require(
        provenance.timing.output_format == entry.output_format,
        format!("{}: output format mismatch", path.display()),
    )?;

    let expected_batch = matches!(entry.mode, ExecutionMode::Batch).then_some(contract.batch_size);
    require(
        provenance.fixed_batch_size == expected_batch,
        format!("{}: fixed batch size mismatch", path.display()),
    )?;

    require(
        provenance.frameworks.len() == 1,
        format!("{}: expected one framework", path.display()),
    )?;
    let framework = &provenance.frameworks[0];
    // The run side bakes the execution mode into xberg framework names (batch cells emit
    // `xberg-<fmt>-<pipeline>-batch`), matching how `extract_framework_and_mode` recovers the
    // base name during aggregation; `entry.framework` is the mode-independent base. Compare the
    // stripped base — mode itself is validated separately via `timing.mode`/`fixed_batch_size`.
    require(
        extract_framework_and_mode(&framework.name).0 == entry.framework,
        format!("{}: framework mismatch", path.display()),
    )?;
    require(
        framework.eligible_documents == eligible_documents,
        format!("{}: fixture count mismatch", path.display()),
    )?;
    require(
        framework.batch_partitions == expected_partitions,
        format!("{}: batch partition mismatch", path.display()),
    )?;
    require(
        framework.ocr_language_policy == declared_ocr_language_policy(&entry.framework),
        format!("{}: OCR language policy mismatch", path.display()),
    )?;

    Ok(())
}

fn validate_results(
    results: &[BenchmarkResult],
    path: &Path,
    entry: &MatrixEntry,
    expected_fixtures: &[&ExpectedFixture],
    iterations: usize,
    cohort: Cohort,
    allow_failures: bool,
) -> Result<()> {
    require(
        results.len() == expected_fixtures.len(),
        format!("{}: result fixture count mismatch", path.display()),
    )?;

    let actual_names: Vec<String> = results
        .iter()
        .map(|result| posix_basename(&result.file_path.to_string_lossy()))
        .collect();
    require(
        actual_names
            .iter()
            .map(String::as_str)
            .eq(expected_fixtures.iter().map(|fixture| fixture.document_name.as_str())),
        format!("{}: result fixture order/content mismatch", path.display()),
    )?;
    let unique_names: HashSet<&String> = actual_names.iter().collect();
    require(
        unique_names.len() == actual_names.len(),
        format!("{}: duplicate fixture results", path.display()),
    )?;

    // OCR usage is contractual only for xberg, the subject under test: the OCR cohort must exercise
    // xberg's OCR pipeline (Used) and every other cohort must not (NotUsed). External competitors
    // self-report OCR usage from their own internals — some are Tesseract-backed yet legitimately
    // skip OCR on a given file (e.g. Tika reading an embedded text layer) — so their reported status
    // is descriptive data we record, not a contract we enforce.
    let expected_ocr = if cohort.expects_ocr() {
        OcrStatus::Used
    } else {
        OcrStatus::NotUsed
    };
    let enforce_ocr_status = entry.framework.starts_with("xberg-");
    for (index, result) in results.iter().enumerate() {
        crate::output::validate_result(result)
            .map_err(|error| contract_error(format!("{}: result {index}: {error}", path.display())))?;
        let expected_fixture = expected_fixtures[index];
        require(
            extract_framework_and_mode(&result.framework).0 == entry.framework,
            format!("{}: result {index} framework mismatch", path.display()),
        )?;
        require(
            result.output_format == entry.output_format,
            format!("{}: result {index} format mismatch", path.display()),
        )?;
        if result.success {
            require(
                result.error_kind == ErrorKind::None,
                format!("{}: result {index} has an error", path.display()),
            )?;
            require(
                result.error_message.is_none(),
                format!("{}: result {index} has an error message", path.display()),
            )?;
            validate_release_quality_contract(
                result.quality.as_ref(),
                entry.output_format,
                expected_fixture.has_quality_ground_truth,
                expected_fixture.has_structural_ground_truth,
                path,
                &format!("result {index}"),
            )?;
        } else {
            require(allow_failures, format!("{}: result {index} failed", path.display()))?;
            require(
                result.error_kind != ErrorKind::None,
                format!("{}: failed result {index} has no error kind", path.display()),
            )?;
            require(
                matches!(
                    result.error_kind,
                    ErrorKind::FrameworkError | ErrorKind::Timeout | ErrorKind::EmptyContent
                ),
                format!(
                    "{}: optional result {index} has an infrastructure error",
                    path.display()
                ),
            )?;
            require(
                result
                    .error_message
                    .as_deref()
                    .is_some_and(|message| !message.is_empty()),
                format!("{}: failed result {index} has no error message", path.display()),
            )?;
        }
        require(
            !enforce_ocr_status || result.ocr_status == expected_ocr,
            format!("{}: result {index} OCR status mismatch", path.display()),
        )?;
        require(
            result.iterations.len() == iterations,
            format!("{}: result {index} iteration count mismatch", path.display()),
        )?;
        let sequential = result
            .iterations
            .iter()
            .enumerate()
            .all(|(expected_index, iteration)| iteration.iteration == expected_index + 1);
        require(
            sequential,
            format!("{}: result {index} iteration order/duplicates mismatch", path.display()),
        )?;
    }
    Ok(())
}

fn validate_release_quality_contract(
    quality: Option<&QualityMetrics>,
    output_format: OutputFormat,
    has_quality_ground_truth: bool,
    has_structural_ground_truth: bool,
    path: &Path,
    context: &str,
) -> Result<()> {
    require(
        quality.is_some() == has_quality_ground_truth,
        format!("{}: {context} quality presence mismatch", path.display()),
    )?;
    if let Some(quality) = quality {
        let expected_sf1 = output_format == OutputFormat::Markdown && has_structural_ground_truth;
        require(
            quality.f1_score_layout.is_some() == expected_sf1,
            format!("{}: {context} SF1 presence mismatch", path.display()),
        )?;
    }
    Ok(())
}

fn supported_fixture_indexes(
    entry: &MatrixEntry,
    contract: &CohortContract,
    ocr_languages: &[Option<String>],
) -> Vec<usize> {
    let supported = crate::adapters::external::declared_supported_formats(&entry.framework);
    contract
        .document_extensions
        .iter()
        .enumerate()
        .filter_map(|(index, extension)| {
            let format_supported =
                entry.framework.starts_with("xberg-") || supported.iter().any(|item| item == extension);
            let language_supported =
                declared_ocr_language_policy(&entry.framework).supports(ocr_languages[index].as_deref());
            (format_supported && language_supported).then_some(index)
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn validate_raw_artifacts(
    artifacts_dir: &Path,
    cohort_manifest: &Path,
    fixtures_root: &Path,
    source_sha: &str,
    run_id: &str,
    iterations: usize,
    cohort: Cohort,
    contract: &CohortContract,
) -> Result<String> {
    let manifest_blake3 = validate_manifest(cohort_manifest, contract)?;
    let fixtures = expected_fixtures(fixtures_root, contract)?;
    let allowed_names: HashMap<String, &MatrixEntry> = contract
        .matrix
        .iter()
        .map(|entry| (format!("{}-{run_id}", entry.artifact), entry))
        .collect();
    let required_names: HashSet<String> = contract
        .matrix
        .iter()
        .filter(|entry| !entry.optional)
        .map(|entry| format!("{}-{run_id}", entry.artifact))
        .collect();
    let actual_dirs = read_artifact_dirs(artifacts_dir)?;

    let allowed_key_set: HashSet<&str> = allowed_names.keys().map(String::as_str).collect();
    let actual_key_set: HashSet<&str> = actual_dirs.keys().map(String::as_str).collect();
    let required_key_set: HashSet<&str> = required_names.iter().map(String::as_str).collect();
    let present_required: HashSet<&str> = actual_key_set.intersection(&required_key_set).copied().collect();
    require(
        required_key_set == present_required,
        describe_set_mismatch(
            "artifacts",
            required_key_set.iter().copied(),
            present_required.iter().copied(),
        ),
    )?;
    require(
        actual_key_set.is_subset(&allowed_key_set),
        describe_set_mismatch(
            "allowed artifacts",
            allowed_key_set.iter().copied(),
            actual_key_set.iter().copied(),
        ),
    )?;

    // Optional means absence is permitted, never that present bytes are trusted. Validate every
    // downloaded optional artifact exactly like a required one, while allowing its recorded
    // extraction failures to remain useful best-effort data. ~keep
    for (artifact_name, artifact_dir) in &actual_dirs {
        let entry = allowed_names[artifact_name];
        let results_path = only_file(artifact_dir, "results.json")?;
        let provenance_path = only_file(artifact_dir, "provenance.json")?;
        let ocr_languages: Vec<Option<String>> = fixtures.iter().map(|fixture| fixture.ocr_language.clone()).collect();
        let supported_indexes = supported_fixture_indexes(entry, contract, &ocr_languages);
        let selected_fixtures: Vec<&ExpectedFixture> =
            supported_indexes.iter().map(|index| &fixtures[*index]).collect();
        let eligible_languages: Vec<Option<String>> = supported_indexes
            .iter()
            .map(|index| ocr_languages[*index].clone())
            .collect();
        let expected_partitions = matches!(entry.mode, ExecutionMode::Batch).then(|| {
            declared_ocr_language_policy(&entry.framework)
                .batch_partition_count(&eligible_languages, contract.batch_size)
        });

        let provenance: RunProvenance = load_typed_json(&provenance_path)?;
        validate_provenance(
            &provenance,
            &provenance_path,
            entry,
            contract,
            &manifest_blake3,
            &fixtures,
            source_sha,
            iterations,
            supported_indexes.len(),
            expected_partitions,
        )?;

        let results: Vec<BenchmarkResult> = load_typed_json(&results_path)?;
        validate_results(
            &results,
            &results_path,
            entry,
            &selected_fixtures,
            iterations,
            cohort,
            entry.optional,
        )?;
    }

    Ok(format!(
        "validated {} {} benchmark artifacts",
        actual_dirs.len(),
        cohort.as_str()
    ))
}

/// Validate one file-type bucket's failure accounting and report its sample count. Required cells
/// must have no errors; optional cells may contain only framework-accountable failures. Cohorts
/// can span several file types, so the caller also checks each bucket against the cohort's exact
/// per-extension fixture counts. ~keep
fn validate_bucket(bucket: &PerformancePercentiles, key: &str, allow_failures: bool) -> Result<usize> {
    let accountable_failures = bucket.framework_errors + bucket.timeouts + bucket.empty_content;
    let infrastructure_failures = bucket.harness_errors + bucket.config_setup_errors;
    let failures = accountable_failures + infrastructure_failures;
    require(
        bucket.successful_sample_count + failures == bucket.total_sample_count,
        format!("{key}: success/error counts do not match total_sample_count"),
    )?;
    require(
        allow_failures || failures == 0,
        format!("{key}: required aggregate contains failures"),
    )?;
    require(
        infrastructure_failures == 0,
        format!("{key}: aggregate contains infrastructure failures"),
    )?;
    Ok(bucket.total_sample_count)
}

fn identity_string(framework: &str, output_format: OutputFormat, mode: &str, fixture_id: &str) -> String {
    // (framework, output_format, mode, fixture_id) already uniquely identifies a row within a cohort.
    // OCR usage is intentionally excluded: it is contractual only for xberg (enforced per-result and
    // per-bucket elsewhere) and merely descriptive for competitors, whose reported side we do not gate.
    format!("{framework}:{output_format}:{mode}:{fixture_id}")
}

fn logical_framework(framework: &str) -> &str {
    if framework.starts_with("xberg-") {
        "xberg"
    } else {
        framework
    }
}

fn validate_format_support(
    aggregate: &NewConsolidatedResults,
    present_entries: &[&MatrixEntry],
    contract: &CohortContract,
    path: &Path,
) -> Result<()> {
    let mut expected_file_types: Vec<String> = contract
        .document_extensions
        .iter()
        .map(|extension| (*extension).to_string())
        .collect();
    expected_file_types.sort();
    expected_file_types.dedup();
    require(
        aggregate.format_support.file_types == expected_file_types,
        format!("{}: format support file types mismatch", path.display()),
    )?;

    let frameworks: std::collections::BTreeSet<&str> = present_entries
        .iter()
        .map(|entry| logical_framework(&entry.framework))
        .collect();
    let mut expected_unsupported = std::collections::BTreeMap::new();
    for framework in frameworks {
        if framework == "xberg" {
            continue;
        }
        let supported = crate::adapters::external::declared_supported_formats(framework);
        let missing: Vec<String> = expected_file_types
            .iter()
            .filter(|extension| !supported.iter().any(|item| item == *extension))
            .cloned()
            .collect();
        if !missing.is_empty() {
            expected_unsupported.insert(framework.to_string(), missing);
        }
    }
    require(
        aggregate.format_support.unsupported == expected_unsupported,
        format!("{}: format support unsupported pairs mismatch", path.display()),
    )
}

fn validate_aggregate_provenance(
    aggregate: &NewConsolidatedResults,
    present_entries: &[&MatrixEntry],
    contract: &CohortContract,
    iterations: usize,
    fixture_expectations: &[FixtureQualityExpectation],
    path: &Path,
) -> Result<()> {
    require(
        aggregate.run_provenance.len() == present_entries.len(),
        format!("{}: aggregate provenance count mismatch", path.display()),
    )?;
    let expected_entries: HashMap<String, &MatrixEntry> = present_entries
        .iter()
        .map(|entry| {
            (
                format!(
                    "{}:{}:{}",
                    entry.framework,
                    entry.output_format,
                    entry.mode.aggregate_slug()
                ),
                *entry,
            )
        })
        .collect();
    let expected_cells: HashSet<String> = expected_entries.keys().cloned().collect();
    let mut actual_cells = HashSet::new();
    let mut source_sha: Option<&str> = None;
    let mut fixture_signature: Option<Vec<(&str, &str, &str, u64)>> = None;
    for record in &aggregate.run_provenance {
        require(
            record.missing_reason.is_none(),
            format!("{}: aggregate provenance contains a missing reason", path.display()),
        )?;
        let provenance = record.provenance.as_ref().ok_or_else(|| {
            contract_error(format!(
                "{}: aggregate provenance missing for {}",
                path.display(),
                record.source_dir
            ))
        })?;
        require(
            provenance.schema_version == EXPECTED_PROVENANCE_SCHEMA_VERSION,
            format!("{}: aggregate provenance schema mismatch", path.display()),
        )?;
        require(
            provenance.repository.dirty == Some(false),
            format!("{}: aggregate provenance records a dirty checkout", path.display()),
        )?;
        let commit =
            provenance.repository.commit.as_deref().ok_or_else(|| {
                contract_error(format!("{}: aggregate provenance has no source commit", path.display()))
            })?;
        require(
            commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
            format!("{}: aggregate provenance has an invalid source commit", path.display()),
        )?;
        require(
            source_sha.is_none_or(|expected| expected == commit),
            format!("{}: aggregate provenance mixes source commits", path.display()),
        )?;
        source_sha.get_or_insert(commit);
        require(
            provenance.corpus.cohort.as_deref() == Some(contract.manifest_name)
                && provenance.corpus.cohort_manifest_blake3.as_deref() == Some(contract.manifest_blake3),
            format!("{}: aggregate provenance cohort mismatch", path.display()),
        )?;
        let signature: Vec<(&str, &str, &str, u64)> = provenance
            .corpus
            .ordered_fixtures
            .iter()
            .map(|fixture| {
                (
                    fixture.fixture.as_str(),
                    fixture.fixture_blake3.as_str(),
                    fixture.document_blake3.as_str(),
                    fixture.document_bytes,
                )
            })
            .collect();
        require(
            signature.len() == contract.fixtures.len()
                && signature
                    .iter()
                    .zip(contract.fixtures)
                    .all(|((fixture, _, _, _), expected)| fixture == expected)
                && fixture_signature.as_ref().is_none_or(|expected| expected == &signature),
            format!("{}: aggregate provenance fixture identity mismatch", path.display()),
        )?;
        fixture_signature.get_or_insert(signature);
        require(
            provenance.frameworks.len() == 1,
            format!("{}: aggregate provenance must contain one framework", path.display()),
        )?;
        let framework = &provenance.frameworks[0];
        let mode = match provenance.timing.mode {
            crate::config::BenchmarkMode::SingleFile => "single",
            crate::config::BenchmarkMode::Batch => "batch",
        };
        let cell = format!(
            "{}:{}:{}",
            extract_framework_and_mode(&framework.name).0,
            provenance.timing.output_format,
            mode
        );
        let entry = expected_entries.get(&cell).ok_or_else(|| {
            contract_error(format!(
                "{}: unexpected aggregate provenance cell {cell}",
                path.display()
            ))
        })?;
        let ocr_languages: Vec<Option<String>> = fixture_expectations
            .iter()
            .map(|fixture| fixture.ocr_language.clone())
            .collect();
        let supported_indexes = supported_fixture_indexes(entry, contract, &ocr_languages);
        let expected_eligible = supported_indexes.len();
        let eligible_languages: Vec<Option<String>> = supported_indexes
            .iter()
            .map(|index| ocr_languages[*index].clone())
            .collect();
        let is_batch = entry.mode == ExecutionMode::Batch;
        require(
            provenance.timing.mode == entry.mode.benchmark_mode()
                && provenance.timing.benchmark_iterations == iterations
                && provenance.fixed_batch_size == is_batch.then_some(contract.batch_size)
                && framework.eligible_documents == expected_eligible
                && framework.batch_partitions
                    == is_batch.then(|| {
                        declared_ocr_language_policy(&entry.framework)
                            .batch_partition_count(&eligible_languages, contract.batch_size)
                    })
                && framework.ocr_language_policy == declared_ocr_language_policy(&entry.framework),
            format!("{}: aggregate provenance settings mismatch for {cell}", path.display()),
        )?;
        actual_cells.insert(cell);
    }
    require(
        actual_cells == expected_cells,
        describe_set_mismatch(
            "aggregate provenance cells",
            expected_cells.iter().map(String::as_str),
            actual_cells.iter().map(String::as_str),
        ),
    )
}

fn error_kind_from_row(row: &PerFixtureRow, path: &Path) -> Result<ErrorKind> {
    match row.error_kind.as_deref() {
        None if row.success => Ok(ErrorKind::None),
        Some("FrameworkError") => Ok(ErrorKind::FrameworkError),
        Some("EmptyContent") => Ok(ErrorKind::EmptyContent),
        Some("Timeout") => Ok(ErrorKind::Timeout),
        Some("HarnessError") => Ok(ErrorKind::HarnessError),
        Some("ConfigSetupError") => Ok(ErrorKind::ConfigSetupError),
        kind => Err(contract_error(format!(
            "{}: invalid fixture-row error kind {kind:?}",
            path.display()
        ))),
    }
}

fn duration_from_millis(value: f64, field: &str, path: &Path) -> Result<Duration> {
    require(
        value.is_finite() && value >= 0.0,
        format!("{}: invalid fixture-row {field}", path.display()),
    )?;
    Ok(Duration::from_secs_f64(value / 1_000.0))
}

fn performance_metrics_from_row(row: &PerFixtureRow, path: &Path) -> Result<PerformanceMetrics> {
    let peak_memory_bytes = row.peak_memory_mb * 1_000_000.0;
    require(
        peak_memory_bytes.is_finite() && peak_memory_bytes >= 0.0 && peak_memory_bytes <= u64::MAX as f64,
        format!("{}: invalid fixture-row peak memory", path.display()),
    )?;
    Ok(PerformanceMetrics {
        baseline_memory_bytes: row.baseline_memory_bytes,
        peak_memory_bytes: peak_memory_bytes.round() as u64,
        peak_memory_delta_bytes: row.peak_memory_delta_bytes,
        avg_cpu_percent: row.avg_cpu_percent,
        cpu_seconds: row.cpu_seconds,
        throughput_bytes_per_sec: row.throughput_bytes_per_sec,
        p50_memory_bytes: row.p50_memory_bytes,
        p95_memory_bytes: row.p95_memory_bytes,
        p99_memory_bytes: row.p99_memory_bytes,
    })
}

fn validate_quality_projection(row: &PerFixtureRow, path: &Path) -> Result<()> {
    if let Some(quality) = &row.quality {
        // This strict format rule is intentionally scoped to the current aggregated release
        // schema. Generic `results.json` loading/writing remains backward-compatible with older
        // captures that populated layout F1 for plaintext rows without a schema version. ~keep
        require(
            row.output_format != OutputFormat::Plaintext || quality.f1_score_layout.is_none(),
            format!(
                "{}: plaintext fixture-row quality must not contain f1_score_layout",
                path.display()
            ),
        )?;
        for (name, value) in [
            ("f1_score_text", quality.f1_score_text),
            ("f1_score_numeric", quality.f1_score_numeric),
            ("quality_score", quality.quality_score),
        ] {
            require(
                value.is_finite() && (0.0..=1.0).contains(&value),
                format!(
                    "{}: fixture-row {name} must be a finite value in [0, 1]",
                    path.display()
                ),
            )?;
        }
        if let Some(value) = quality.f1_score_layout {
            require(
                value.is_finite() && (0.0..=1.0).contains(&value),
                format!(
                    "{}: fixture-row f1_score_layout must be a finite value in [0, 1]",
                    path.display()
                ),
            )?;
        }
    }
    let quality_projection = row.quality.as_ref().map(|quality| {
        (
            Some(quality.f1_score_text),
            quality.f1_score_layout,
            Some(quality.f1_score_numeric),
            Some(quality.quality_score),
            Some(quality.correct),
        )
    });
    require(
        quality_projection.unwrap_or((None, None, None, None, None))
            == (
                row.f1_text,
                row.f1_layout,
                row.f1_numeric,
                row.quality_score,
                row.correct,
            ),
        format!("{}: fixture-row quality projection mismatch", path.display()),
    )
}

fn benchmark_result_from_row(row: &PerFixtureRow, path: &Path) -> Result<BenchmarkResult> {
    validate_quality_projection(row, path)?;
    let framework = match row.execution_mode.as_str() {
        "batch" => format!("{}-batch", row.framework),
        _ => row.framework.clone(),
    };
    let ocr_status = match row.ocr {
        Some(true) => OcrStatus::Used,
        Some(false) => OcrStatus::NotUsed,
        None => OcrStatus::Unknown,
    };
    let extraction_duration = row
        .extraction_duration_ms
        .map(|value| duration_from_millis(value, "extraction duration", path))
        .transpose()?;
    let subprocess_overhead = row
        .subprocess_overhead_ms
        .map(|value| duration_from_millis(value, "subprocess overhead", path))
        .transpose()?;
    let cold_start_duration = row
        .cold_start_duration_ms
        .map(|value| duration_from_millis(value, "cold start duration", path))
        .transpose()?;
    Ok(BenchmarkResult {
        framework,
        output_format: row.output_format,
        file_path: PathBuf::from(format!("{}.{}", row.fixture_id, row.file_type)),
        file_size: row.file_size,
        success: row.success,
        error_message: row.error_message.clone(),
        error_kind: error_kind_from_row(row, path)?,
        duration: duration_from_millis(row.duration_ms, "duration", path)?,
        extraction_duration,
        subprocess_overhead,
        metrics: performance_metrics_from_row(row, path)?,
        quality: row.quality.clone(),
        iterations: row.iterations.clone(),
        statistics: row.statistics.clone(),
        cold_start_duration,
        file_extension: row.file_type.clone(),
        framework_capabilities: row.framework_capabilities.clone(),
        pdf_metadata: row.pdf_metadata.clone(),
        ocr_status,
        extracted_text: None,
        system_load: row.system_load,
    })
}

fn require_serialized_equal<T: Serialize>(actual: &T, expected: &T, label: &str, path: &Path) -> Result<()> {
    let actual = serde_json::to_value(actual)
        .map_err(|error| contract_error(format!("{}: serialize {label}: {error}", path.display())))?;
    let expected = serde_json::to_value(expected)
        .map_err(|error| contract_error(format!("{}: serialize expected {label}: {error}", path.display())))?;
    require(actual == expected, format!("{}: {label} mismatch", path.display()))
}

fn validate_derived_aggregate_fields(
    aggregate: &NewConsolidatedResults,
    rows: &[&PerFixtureRow],
    cohort: Cohort,
    path: &Path,
) -> Result<()> {
    let results: Vec<BenchmarkResult> = rows
        .iter()
        .map(|row| benchmark_result_from_row(row, path))
        .collect::<Result<_>>()?;
    let rebuilt = crate::aggregate::aggregate_new_format(&results);
    require_serialized_equal(
        &aggregate.by_framework_mode,
        &rebuilt.by_framework_mode,
        "aggregate metrics",
        path,
    )?;
    require_serialized_equal(&aggregate.disk_sizes, &rebuilt.disk_sizes, "disk sizes", path)?;
    require_serialized_equal(
        &aggregate.format_support,
        &rebuilt.format_support,
        "format support",
        path,
    )?;
    require_serialized_equal(
        &aggregate.failure_summary,
        &rebuilt.failure_summary,
        "failure summary",
        path,
    )?;
    let expected_comparison = comparison_for_cohort(&rebuilt.by_framework_mode, cohort);
    require_serialized_equal(&aggregate.comparison, &expected_comparison, "comparison rankings", path)?;
    require(
        aggregate.metadata.total_results == rebuilt.metadata.total_results
            && aggregate.metadata.framework_count == rebuilt.metadata.framework_count
            && aggregate.metadata.file_type_count == rebuilt.metadata.file_type_count
            && aggregate.metadata.shared_corpus_markdown == rebuilt.metadata.shared_corpus_markdown
            && aggregate.metadata.shared_corpus_plaintext == rebuilt.metadata.shared_corpus_plaintext
            && chrono::DateTime::parse_from_rfc3339(&aggregate.metadata.timestamp).is_ok()
            && aggregate.metadata.disk_size_conflicts == rebuilt.metadata.disk_size_conflicts,
        format!("{}: consolidation metadata mismatch", path.display()),
    )
}

fn validate_aggregate(
    path: &Path,
    cohort: Cohort,
    contract: &CohortContract,
    iterations: usize,
    quality_expectations: &[FixtureQualityExpectation],
) -> Result<String> {
    let aggregate: NewConsolidatedResults = load_typed_json(path)?;
    require(
        aggregate.schema_version == SCHEMA_VERSION,
        format!("{}: unexpected schema", path.display()),
    )?;

    let required_entries: Vec<&MatrixEntry> = contract.matrix.iter().filter(|entry| !entry.optional).collect();
    let allowed_entries: HashMap<String, &MatrixEntry> = contract
        .matrix
        .iter()
        .map(|entry| (entry.aggregate_key(), entry))
        .collect();

    let expected_keys: HashSet<String> = required_entries.iter().map(|entry| entry.aggregate_key()).collect();
    let allowed_keys: HashSet<&str> = allowed_entries.keys().map(String::as_str).collect();
    let actual_keys: HashSet<&str> = aggregate.by_framework_mode.keys().map(String::as_str).collect();
    let required_keys: HashSet<&str> = expected_keys.iter().map(String::as_str).collect();
    let present_required: HashSet<&str> = actual_keys.intersection(&required_keys).copied().collect();
    require(
        required_keys == present_required,
        describe_set_mismatch(
            "aggregate keys",
            required_keys.iter().copied(),
            present_required.iter().copied(),
        ),
    )?;
    require(
        actual_keys.is_subset(&allowed_keys),
        describe_set_mismatch(
            "allowed aggregate keys",
            allowed_keys.iter().copied(),
            actual_keys.iter().copied(),
        ),
    )?;

    let present_entries: Vec<&MatrixEntry> = actual_keys.iter().map(|key| allowed_entries[*key]).collect();
    validate_format_support(&aggregate, &present_entries, contract, path)?;
    validate_aggregate_provenance(
        &aggregate,
        &present_entries,
        contract,
        iterations,
        quality_expectations,
        path,
    )?;

    let expects_ocr = cohort.expects_ocr();
    // A present best-effort group must retain exact supported-format cardinality and valid failure
    // accounting; only its absence and well-categorized extraction failures are optional. ~keep
    for (key, group) in &aggregate.by_framework_mode {
        let entry = allowed_entries[key];
        require(
            !group.by_file_type.is_empty(),
            format!("{}: group {key} has no file-type metrics", path.display()),
        )?;
        // xberg (the subject under test) has a uniform OCR expectation, so its fixtures must all sit
        // on the cohort's expected OCR side with the opposite side empty, and its per-extension sample
        // cardinality must match the contract exactly. Competitors self-report OCR usage from their own
        // internals — a single framework can straddle both sides across a cohort, and a failed
        // extraction reports Unknown (in neither OCR bucket) — so we only integrity-check whatever
        // buckets they populated; their fixture coverage is enforced exactly by the per-fixture-row
        // identity set below, which is OCR-status agnostic.
        if entry.framework.starts_with("xberg-") {
            let mut actual_ext_counts: HashMap<&str, usize> = HashMap::new();
            for (file_type, file_group) in &group.by_file_type {
                let (present, absent) = if expects_ocr {
                    (&file_group.with_ocr, &file_group.no_ocr)
                } else {
                    (&file_group.no_ocr, &file_group.with_ocr)
                };
                require(
                    absent.is_none(),
                    format!(
                        "{}: group {key} file type {file_type} has wrong OCR bucket",
                        path.display()
                    ),
                )?;
                let bucket = present.as_ref().ok_or_else(|| {
                    contract_error(format!(
                        "{}: group {key} file type {file_type} missing OCR bucket",
                        path.display()
                    ))
                })?;
                actual_ext_counts.insert(file_type.as_str(), validate_bucket(bucket, key, entry.optional)?);
            }
            let ocr_languages: Vec<Option<String>> = quality_expectations
                .iter()
                .map(|fixture| fixture.ocr_language.clone())
                .collect();
            let supported_indexes = supported_fixture_indexes(entry, contract, &ocr_languages);
            let mut entry_ext_counts: HashMap<&str, usize> = HashMap::new();
            for index in supported_indexes {
                *entry_ext_counts.entry(contract.document_extensions[index]).or_insert(0) += 1;
            }
            let expected_set: HashSet<&str> = entry_ext_counts.keys().copied().collect();
            let actual_set: HashSet<&str> = actual_ext_counts.keys().copied().collect();
            require(
                expected_set == actual_set,
                format!(
                    "{}: group {key} {}",
                    path.display(),
                    describe_set_mismatch(
                        "file-type buckets",
                        expected_set.iter().copied(),
                        actual_set.iter().copied()
                    )
                ),
            )?;
            for (extension, expected_count) in &entry_ext_counts {
                let actual = actual_ext_counts.get(extension).copied().unwrap_or(0);
                require(
                    actual == *expected_count,
                    format!(
                        "{}: group {key} file type {extension} covers {actual} samples, expected {expected_count}",
                        path.display()
                    ),
                )?;
            }
        } else {
            for file_group in group.by_file_type.values() {
                for side in [&file_group.no_ocr, &file_group.with_ocr].into_iter().flatten() {
                    validate_bucket(side, key, entry.optional)?;
                }
            }
        }
    }

    let rows: Vec<&PerFixtureRow> = aggregate.per_fixture_results.iter().collect();
    let ocr_languages: Vec<Option<String>> = quality_expectations
        .iter()
        .map(|fixture| fixture.ocr_language.clone())
        .collect();
    let expected_identities: HashSet<String> = present_entries
        .iter()
        .flat_map(|entry| {
            supported_fixture_indexes(entry, contract, &ocr_languages)
                .into_iter()
                .map(move |index| {
                    identity_string(
                        &entry.framework,
                        entry.output_format,
                        entry.mode.aggregate_slug(),
                        contract.document_stems[index],
                    )
                })
        })
        .collect();
    let expected_row_count = expected_identities.len();
    require(
        rows.len() == expected_row_count,
        format!("{}: expected {expected_row_count} fixture rows", path.display()),
    )?;

    let identities: HashSet<String> = rows
        .iter()
        .map(|row| identity_string(&row.framework, row.output_format, &row.execution_mode, &row.fixture_id))
        .collect();
    require(
        identities == expected_identities,
        describe_set_mismatch(
            "aggregate fixture rows",
            expected_identities.iter().map(String::as_str),
            identities.iter().map(String::as_str),
        ),
    )?;

    for row in &rows {
        let key = crate::aggregate::make_aggregate_key(&row.framework, row.output_format, &row.execution_mode);
        let entry = allowed_entries
            .get(&key)
            .ok_or_else(|| contract_error(format!("{}: row has no matching aggregate key", path.display())))?;
        let fixture_index = contract
            .document_stems
            .iter()
            .position(|stem| *stem == row.fixture_id)
            .ok_or_else(|| contract_error(format!("{}: row has unknown fixture identity", path.display())))?;
        let quality_expectation = &quality_expectations[fixture_index];
        if row.success {
            require(
                row.error_kind.is_none() && row.error_message.is_none(),
                format!("{}: successful fixture row contains an error", path.display()),
            )?;
            validate_release_quality_contract(
                row.quality.as_ref(),
                row.output_format,
                quality_expectation.has_quality_ground_truth,
                quality_expectation.has_structural_ground_truth,
                path,
                &format!("fixture row {}", row.fixture_id),
            )?;
        } else {
            require(
                entry.optional,
                format!("{}: failed required fixture row", path.display()),
            )?;
            require(
                row.error_kind
                    .as_deref()
                    .is_some_and(|kind| matches!(kind, "FrameworkError" | "Timeout" | "EmptyContent")),
                format!(
                    "{}: failed fixture rows: optional row has no accountable error kind",
                    path.display()
                ),
            )?;
            require(
                row.error_message.as_deref().is_some_and(|message| !message.is_empty()),
                format!(
                    "{}: failed fixture rows: optional row has no error message",
                    path.display()
                ),
            )?;
        }
    }

    // The group counters and per-fixture rows are two published views of the same failures. Check
    // them against each other so a well-typed but internally inconsistent aggregate cannot pass. ~keep
    for (key, group) in &aggregate.by_framework_mode {
        for (file_type, file_group) in &group.by_file_type {
            // Reconcile each populated OCR side against exactly the rows that fed it (no_ocr ← NotUsed,
            // with_ocr ← Used). Splitting by side lets a competitor legitimately straddle both without
            // double-counting, while xberg's single expected side reconciles as before.
            for (side_is_with_ocr, bucket) in [(false, &file_group.no_ocr), (true, &file_group.with_ocr)] {
                let Some(bucket) = bucket.as_ref() else { continue };
                let bucket_rows: Vec<&&PerFixtureRow> = rows
                    .iter()
                    .filter(|row| {
                        crate::aggregate::make_aggregate_key(&row.framework, row.output_format, &row.execution_mode)
                            == *key
                            && row.file_type == *file_type
                            && row.ocr == Some(side_is_with_ocr)
                    })
                    .collect();
                let count_kind = |kind: &str| {
                    bucket_rows
                        .iter()
                        .filter(|row| !row.success && row.error_kind.as_deref() == Some(kind))
                        .count()
                };
                require(
                    bucket.successful_sample_count == bucket_rows.iter().filter(|row| row.success).count()
                        && bucket.framework_errors == count_kind("FrameworkError")
                        && bucket.harness_errors == count_kind("HarnessError")
                        && bucket.config_setup_errors == count_kind("ConfigSetupError")
                        && bucket.timeouts == count_kind("Timeout")
                        && bucket.empty_content == count_kind("EmptyContent"),
                    format!("{key}: group failure counts do not match fixture rows for {file_type}"),
                )?;
            }
        }
    }
    validate_derived_aggregate_fields(&aggregate, &rows, cohort, path)?;

    Ok(format!(
        "validated {} {} aggregate keys and {} fixture rows",
        actual_keys.len(),
        cohort.as_str(),
        rows.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_fixture_hashing_rejects_document_path_escape() {
        const FIXTURES: &[&str] = &["escape.json"];
        const STEMS: &[&str] = &["secret"];
        const EXTENSIONS: &[&str] = &["txt"];

        let root = tempfile::tempdir().unwrap();
        let fixtures_root = root.path().join("fixtures");
        std::fs::create_dir(&fixtures_root).unwrap();
        std::fs::write(root.path().join("secret.txt"), "outside fixture boundary").unwrap();
        std::fs::write(
            fixtures_root.join("escape.json"),
            serde_json::json!({
                "document": "../secret.txt",
                "file_type": "txt",
                "file_size": 24,
                "expected_frameworks": [],
                "metadata": {}
            })
            .to_string(),
        )
        .unwrap();
        let contract = CohortContract {
            manifest_name: "test",
            manifest_blake3: "unused",
            batch_size: 1,
            fixtures: FIXTURES,
            document_stems: STEMS,
            document_extensions: EXTENSIONS,
            matrix: Vec::new(),
        };

        let error = expected_fixtures(&fixtures_root, &contract).unwrap_err().to_string();

        assert!(error.contains("document escapes the fixture trust boundary"), "{error}");
    }
}
