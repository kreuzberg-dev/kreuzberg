//! Reproducibility metadata for benchmark runs.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::adapter::FrameworkAdapter;
use crate::config::{BenchmarkConfig, BenchmarkMode};
use crate::fixture::FixtureManager;
use crate::types::{BatchCapability, BatchEntryPoint, OutputFormat};
use crate::{CohortManifest, Error, Result};

const PROVENANCE_SCHEMA_VERSION: u32 = 2;

/// A path-free executable identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutableProvenance {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
    /// Digest of command arguments and any argument that resolves to a file.
    pub invocation_blake3: String,
}

impl ExecutableProvenance {
    pub fn from_command(command: &Path) -> Self {
        Self::from_invocation(command, &[])
    }

    pub fn from_invocation(command: &Path, args: &[String]) -> Self {
        let resolved_command = command
            .is_file()
            .then(|| command.to_path_buf())
            .or_else(|| which::which(command).ok());
        let mut invocation = blake3::Hasher::new();
        for arg in args {
            let argument_path = Path::new(arg);
            if argument_path.is_file() {
                invocation.update(b"file:");
                if let Some(name) = argument_path.file_name() {
                    invocation.update(name.to_string_lossy().as_bytes());
                }
                if let Ok(digest) = hash_file(argument_path) {
                    invocation.update(digest.as_bytes());
                }
            } else {
                invocation.update(&(arg.len() as u64).to_le_bytes());
                invocation.update(arg.as_bytes());
            }
        }
        Self {
            name: command
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string(),
            blake3: resolved_command.as_deref().map(hash_file).transpose().ok().flatten(),
            invocation_blake3: invocation.finalize().to_hex().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelProvenance {
    pub framework: String,
    /// Stable model identifier, preferably `repository@revision#digest`.
    pub identifier: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryProvenance {
    pub commit: Option<String>,
    pub dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureProvenance {
    pub fixture: String,
    pub fixture_blake3: String,
    pub document_blake3: String,
    pub document_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorpusProvenance {
    pub cohort: Option<String>,
    pub cohort_manifest_blake3: Option<String>,
    pub ordered_fixtures: Vec<FixtureProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameworkProvenance {
    pub name: String,
    pub version: String,
    pub executable: Option<ExecutableProvenance>,
    pub models: Vec<String>,
    pub batch_capability: Option<BatchCapability>,
    pub requested_workers: Option<usize>,
    pub effective_workers: Option<usize>,
    /// Configured thread budget for Xberg native batch runs.
    ///
    /// This is distinct from the document concurrency cap in
    /// [`Self::requested_workers`] and is unavailable for other frameworks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configured_thread_budget: Option<usize>,
    pub worker_semantics: String,
    pub effective_warmup_iterations: usize,
    pub eligible_documents: usize,
    pub batch_partitions: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimingProvenance {
    pub mode: BenchmarkMode,
    pub warmup_iterations: usize,
    pub benchmark_iterations: usize,
    pub timeout_ms: u128,
    pub output_format: OutputFormat,
}

/// Sidecar metadata for a standard `run` invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunProvenance {
    pub schema_version: u32,
    pub harness_version: String,
    pub repository: RepositoryProvenance,
    pub corpus: CorpusProvenance,
    pub frameworks: Vec<FrameworkProvenance>,
    pub timing: TimingProvenance,
    pub fixed_batch_size: Option<usize>,
}

/// Inputs used to capture a run's provenance before framework execution.
pub struct ProvenanceInputs<'a> {
    pub config: &'a BenchmarkConfig,
    pub output_format: OutputFormat,
    pub fixture_root: &'a Path,
    pub fixtures: &'a FixtureManager,
    pub frameworks: &'a [Arc<dyn FrameworkAdapter>],
    pub cohort: Option<&'a CohortManifest>,
    pub cohort_manifest_path: Option<&'a Path>,
    pub fixed_batch_size: Option<usize>,
    pub models: &'a [ModelProvenance],
}

impl RunProvenance {
    pub fn capture(inputs: ProvenanceInputs<'_>) -> Result<Self> {
        let corpus = capture_corpus(
            inputs.fixture_root,
            inputs.fixtures,
            inputs.cohort,
            inputs.cohort_manifest_path,
        )?;
        let models = inputs
            .models
            .iter()
            .fold(HashMap::<&str, Vec<String>>::new(), |mut map, model| {
                map.entry(&model.framework).or_default().push(model.identifier.clone());
                map
            });
        for model in inputs.models {
            if !inputs
                .frameworks
                .iter()
                .any(|adapter| adapter.name() == model.framework)
            {
                return Err(Error::Config(format!(
                    "model identity names unselected framework '{}'",
                    model.framework
                )));
            }
        }
        let mut frameworks = Vec::with_capacity(inputs.frameworks.len());
        for adapter in inputs.frameworks {
            let capability = matches!(inputs.config.benchmark_mode, BenchmarkMode::Batch)
                .then(|| adapter.batch_capability())
                .flatten();
            let eligible_documents = inputs
                .fixtures
                .fixtures()
                .iter()
                .filter(|(_, fixture)| {
                    adapter.supports_format(&fixture.file_type)
                        && fixture
                            .document
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_none_or(|name| !adapter.should_skip_file(name))
                })
                .count();
            let batch_partitions = inputs
                .fixed_batch_size
                .filter(|_| capability.is_some())
                .map(|size| {
                    if eligible_documents == 0 {
                        return Err(Error::Config(format!(
                            "framework '{}' has no eligible documents in the fixed cohort",
                            adapter.name()
                        )));
                    }
                    if !eligible_documents.is_multiple_of(size) {
                        return Err(Error::Config(format!(
                            "framework '{}' has {eligible_documents} eligible documents, not a complete multiple of fixed batch size {size}",
                            adapter.name()
                        )));
                    }
                    Ok(eligible_documents / size)
                })
                .transpose()?;
            let batch_workers = capability.map(|_| adapter.worker_provenance(inputs.config.max_concurrent));
            let (requested_workers, effective_workers) = worker_counts(
                inputs.config.benchmark_mode,
                inputs.config.max_concurrent,
                batch_workers,
            );
            let configured_thread_budget =
                configured_thread_budget(inputs.config.benchmark_mode, capability, adapter.as_ref());
            frameworks.push(FrameworkProvenance {
                name: adapter.name().to_string(),
                version: adapter.version(),
                executable: adapter.executable_provenance_for_mode(inputs.config.benchmark_mode),
                models: models.get(adapter.name()).cloned().unwrap_or_default(),
                batch_capability: capability,
                requested_workers,
                effective_workers,
                configured_thread_budget,
                worker_semantics: worker_semantics(inputs.config.benchmark_mode, capability).to_string(),
                effective_warmup_iterations: capability.map_or(inputs.config.warmup_iterations, |value| {
                    if value.timing_scope == crate::types::BatchTimingScope::ColdEndToEndSubprocess {
                        0
                    } else {
                        inputs.config.warmup_iterations
                    }
                }),
                eligible_documents,
                batch_partitions,
            });
        }

        Ok(Self {
            schema_version: PROVENANCE_SCHEMA_VERSION,
            harness_version: env!("CARGO_PKG_VERSION").to_string(),
            repository: capture_repository(inputs.fixture_root),
            corpus,
            frameworks,
            timing: TimingProvenance {
                mode: inputs.config.benchmark_mode,
                warmup_iterations: inputs.config.warmup_iterations,
                benchmark_iterations: inputs.config.benchmark_iterations,
                timeout_ms: inputs.config.timeout.as_millis(),
                output_format: inputs.output_format,
            },
            fixed_batch_size: inputs.fixed_batch_size,
        })
    }
}

pub fn write_run_provenance(provenance: &RunProvenance, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(provenance)?)?;
    Ok(())
}

fn capture_corpus(
    fixture_root: &Path,
    fixtures: &FixtureManager,
    cohort: Option<&CohortManifest>,
    cohort_manifest_path: Option<&Path>,
) -> Result<CorpusProvenance> {
    let mut ordered_fixtures = Vec::with_capacity(fixtures.len());
    for (fixture_path, fixture) in fixtures.fixtures() {
        let fixture_dir = fixture_path.parent().unwrap_or_else(|| Path::new("."));
        let document_path = fixture.resolve_document_path(fixture_dir);
        ordered_fixtures.push(FixtureProvenance {
            fixture: relative_identity(fixture_root, fixture_path),
            fixture_blake3: hash_file(fixture_path)?,
            document_blake3: hash_file(&document_path)?,
            document_bytes: std::fs::metadata(&document_path)?.len(),
        });
    }
    Ok(CorpusProvenance {
        cohort: cohort.map(|manifest| manifest.name.clone()),
        cohort_manifest_blake3: cohort_manifest_path.map(hash_file).transpose()?,
        ordered_fixtures,
    })
}

fn relative_identity(root: &Path, path: &Path) -> String {
    let relative = path
        .canonicalize()
        .ok()
        .and_then(|path| {
            root.canonicalize()
                .ok()
                .and_then(|root| path.strip_prefix(root).ok().map(PathBuf::from))
        })
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| path.file_name().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("unknown"));
    relative.to_string_lossy().replace('\\', "/")
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn capture_repository(start_directory: &Path) -> RepositoryProvenance {
    let repository_root = git_output(start_directory, &["rev-parse", "--show-toplevel"]).map(PathBuf::from);
    let Some(repository_root) = repository_root else {
        return RepositoryProvenance {
            commit: None,
            dirty: None,
        };
    };
    let commit = git_output(&repository_root, &["rev-parse", "HEAD"]);
    let dirty = git_output_bytes(&repository_root, &["status", "--porcelain"]).map(|output| !output.is_empty());
    RepositoryProvenance { commit, dirty }
}

fn git_output(repository_root: &Path, args: &[&str]) -> Option<String> {
    git_output_bytes(repository_root, args)
        .and_then(|output| String::from_utf8(output).ok())
        .map(|value| value.trim().to_string())
}

fn git_output_bytes(repository_root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    Command::new("git")
        .arg("-C")
        .arg(repository_root)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
}

fn worker_counts(
    mode: BenchmarkMode,
    configured: usize,
    batch_workers: Option<(Option<usize>, Option<usize>)>,
) -> (Option<usize>, Option<usize>) {
    match (mode, batch_workers) {
        (BenchmarkMode::Batch, Some(workers)) => workers,
        (BenchmarkMode::SingleFile, _) => (Some(configured), Some(1)),
        (BenchmarkMode::Batch, None) => (Some(configured), Some(configured)),
    }
}

fn worker_semantics(mode: BenchmarkMode, capability: Option<BatchCapability>) -> &'static str {
    match (mode, capability.map(|value| value.entry_point)) {
        (BenchmarkMode::SingleFile, _) => "sequential single-file execution",
        (BenchmarkMode::Batch, Some(BatchEntryPoint::XbergCliExtractBatch)) => {
            "configured document concurrency cap; Xberg thread budget is recorded separately"
        }
        (BenchmarkMode::Batch, Some(BatchEntryPoint::DoclingConvertAll)) => {
            "convert_all document stream; adapter does not override Docling workers"
        }
        (BenchmarkMode::Batch, Some(BatchEntryPoint::LiteparseBatchParse)) => {
            "OCR page workers; not document-level concurrency"
        }
        (BenchmarkMode::Batch, Some(BatchEntryPoint::MineruDoParse)) => {
            "MinerU 3.4.4 do_parse pipeline; model batches span documents through doc_analyze_streaming processing windows"
        }
        (BenchmarkMode::Batch, None) => "batch harness concurrency",
    }
}

fn configured_thread_budget(
    mode: BenchmarkMode,
    capability: Option<BatchCapability>,
    adapter: &dyn FrameworkAdapter,
) -> Option<usize> {
    matches!(
        (mode, capability.map(|value| value.entry_point)),
        (BenchmarkMode::Batch, Some(BatchEntryPoint::XbergCliExtractBatch))
    )
    .then(|| adapter.configured_thread_budget())
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_identity_never_contains_its_parent_path() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("private-binary");
        std::fs::write(&executable, b"binary").unwrap();
        let identity = ExecutableProvenance::from_command(&executable);
        let json = serde_json::to_string(&identity).unwrap();
        assert_eq!(identity.name, "private-binary");
        assert!(!json.contains(temp.path().to_string_lossy().as_ref()));
        assert!(identity.blake3.is_some());
    }

    #[test]
    fn relative_identity_does_not_leak_fixture_root() {
        let temp = tempfile::tempdir().unwrap();
        let fixture = temp.path().join("nested").join("fixture.json");
        std::fs::create_dir_all(fixture.parent().unwrap()).unwrap();
        std::fs::write(&fixture, b"{}").unwrap();
        assert_eq!(relative_identity(temp.path(), &fixture), "nested/fixture.json");
        assert_eq!(relative_identity(&fixture, &fixture), "fixture.json");
    }

    #[test]
    fn repository_provenance_uses_runtime_checkout() {
        let temp = tempfile::tempdir().unwrap();
        let repository = temp.path().join("runtime-checkout");
        let nested = repository.join("target").join("release");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(repository.join("tracked.txt"), b"clean").unwrap();

        for args in [
            vec!["init"],
            vec!["config", "user.email", "benchmark@example.com"],
            vec!["config", "user.name", "Benchmark Test"],
            vec!["add", "tracked.txt"],
            vec!["commit", "-m", "initial"],
        ] {
            assert!(
                Command::new("git")
                    .arg("-C")
                    .arg(&repository)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }

        let clean = capture_repository(&nested);
        assert!(clean.commit.is_some());
        assert_eq!(clean.dirty, Some(false));

        std::fs::write(repository.join("tracked.txt"), b"dirty").unwrap();
        assert_eq!(capture_repository(&nested).dirty, Some(true));
    }

    #[test]
    fn single_file_worker_provenance_is_sequential() {
        assert_eq!(
            worker_counts(BenchmarkMode::SingleFile, 8, Some((Some(8), Some(8)))),
            (Some(8), Some(1))
        );
        assert_eq!(
            worker_semantics(BenchmarkMode::SingleFile, None),
            "sequential single-file execution"
        );
    }

    #[test]
    fn xberg_batch_worker_semantics_do_not_claim_dynamic_concurrency() {
        assert_eq!(
            worker_semantics(
                BenchmarkMode::Batch,
                Some(BatchCapability {
                    entry_point: BatchEntryPoint::XbergCliExtractBatch,
                    timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                    per_item_timing: true,
                })
            ),
            "configured document concurrency cap; Xberg thread budget is recorded separately"
        );
    }

    #[test]
    fn mineru_batch_provenance_labels_cross_document_model_batching() {
        assert_eq!(
            worker_semantics(
                BenchmarkMode::Batch,
                Some(BatchCapability {
                    entry_point: BatchEntryPoint::MineruDoParse,
                    timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                    per_item_timing: false,
                })
            ),
            "MinerU 3.4.4 do_parse pipeline; model batches span documents through doc_analyze_streaming processing windows"
        );
    }

    #[test]
    fn xberg_batch_provenance_uses_actual_adapter_thread_budget() {
        use crate::adapters::subprocess::SubprocessAdapter;

        let capability = BatchCapability {
            entry_point: BatchEntryPoint::XbergCliExtractBatch,
            timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing: true,
        };
        let mismatched_config = BenchmarkConfig {
            max_concurrent: 2,
            xberg_max_threads: Some(4),
            ..Default::default()
        };
        let adapter = SubprocessAdapter::with_batch_capability(
            "xberg-test",
            "echo",
            vec![],
            vec![],
            vec!["pdf".to_string()],
            capability,
        )
        .with_batch_workers(2)
        .with_xberg_max_threads(8);

        assert_eq!(
            configured_thread_budget(BenchmarkMode::Batch, Some(capability), &adapter),
            Some(8)
        );
        assert_ne!(adapter.configured_thread_budget(), mismatched_config.xberg_max_threads);
        assert_eq!(
            configured_thread_budget(BenchmarkMode::SingleFile, Some(capability), &adapter),
            None
        );
    }

    #[test]
    fn xberg_batch_provenance_agrees_with_legacy_fallback() {
        use crate::adapters::subprocess::SubprocessAdapter;

        let capability = BatchCapability {
            entry_point: BatchEntryPoint::XbergCliExtractBatch,
            timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing: true,
        };
        let config = BenchmarkConfig {
            max_concurrent: 3,
            ..Default::default()
        };
        let adapter = SubprocessAdapter::with_batch_capability(
            "xberg-test",
            "echo",
            vec![],
            vec![],
            vec!["pdf".to_string()],
            capability,
        )
        .with_batch_workers(3);

        assert_eq!(
            configured_thread_budget(BenchmarkMode::Batch, Some(capability), &adapter),
            Some(config.max_concurrent)
        );
    }

    #[test]
    fn non_xberg_batch_provenance_has_no_thread_budget() {
        use crate::adapters::subprocess::SubprocessAdapter;

        let capability = BatchCapability {
            entry_point: BatchEntryPoint::DoclingConvertAll,
            timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing: false,
        };
        let adapter = SubprocessAdapter::with_batch_capability(
            "docling",
            "python",
            vec![],
            vec![],
            vec!["pdf".to_string()],
            capability,
        )
        .with_batch_workers(3)
        .with_xberg_max_threads(8);

        assert_eq!(adapter.configured_thread_budget(), None);
        assert_eq!(
            configured_thread_budget(BenchmarkMode::Batch, Some(capability), &adapter),
            None
        );
    }

    #[test]
    fn old_framework_provenance_deserializes_without_thread_budget() {
        let provenance: FrameworkProvenance = serde_json::from_value(serde_json::json!({
            "name": "xberg-markdown-baseline-batch",
            "version": "1.0.0",
            "executable": null,
            "models": [],
            "batch_capability": null,
            "requested_workers": 4,
            "effective_workers": null,
            "worker_semantics": "legacy",
            "effective_warmup_iterations": 0,
            "eligible_documents": 4,
            "batch_partitions": 1
        }))
        .unwrap();

        assert_eq!(provenance.configured_thread_budget, None);
    }
}
