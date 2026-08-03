//! Benchmark runner for executing and collecting results
//!
//! This module orchestrates benchmark execution across multiple fixtures and frameworks,
//! with support for concurrent execution and progress reporting.

use crate::adapter::FrameworkAdapter;
use crate::config::{BenchmarkConfig, BenchmarkMode};
use crate::fixture::FixtureManager;
use crate::registry::AdapterRegistry;
use crate::stats::percentile_r7;
use crate::system_load::SystemLoad;
use crate::types::{
    BatchCapability, BatchTimingScope, BenchmarkResult, DiskSizeInfo, DurationStatistics, ErrorKind, IterationResult,
    OutputFormat, PerformanceMetrics,
};
use crate::{Error, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

type SingleBenchmarkTask = (PathBuf, String, Arc<dyn FrameworkAdapter>, bool, Option<String>);

fn effective_batch_warmup_iterations(capability: BatchCapability, configured: usize) -> usize {
    if capability.timing_scope == BatchTimingScope::ColdEndToEndSubprocess {
        0
    } else {
        configured
    }
}

#[cfg(feature = "profiling")]
use crate::profile_report::ProfileReport;
#[cfg(feature = "profiling")]
use crate::profiling::ProfileGuard;

/// Calculate amplified iteration count for profiling when needed
///
/// When profiling is enabled, tasks can be amplified (repeated) to increase the
/// profiling duration and collect more samples. This function determines how many
/// times to repeat the task based on its estimated duration to reach a target
/// profiling duration.
///
/// # Arguments
/// * `estimated_duration_ms` - Estimated task duration in milliseconds
/// * `target_profile_duration_ms` - Target minimum profiling duration (default 1000ms)
///
/// # Returns
/// Number of amplified iterations (minimum 1)
fn calculate_amplified_iterations(estimated_duration_ms: u64, target_profile_duration_ms: u64) -> usize {
    if estimated_duration_ms == 0 {
        return 1;
    }

    let amplification = (target_profile_duration_ms as f64 / estimated_duration_ms as f64).ceil() as usize;
    amplification.max(1)
}

fn validate_ocr_cohort(ocr_enabled: bool, ocr_required_count: usize) -> Result<()> {
    if !ocr_enabled && ocr_required_count > 0 {
        return Err(Error::Config(format!(
            "OCR is disabled, but the selected fixture cohort contains {ocr_required_count} OCR-required fixture(s). \
             Rerun with --ocr or select a fixture directory/shard containing only non-OCR fixtures; \
             the harness will not silently omit OCR-required documents."
        )));
    }
    Ok(())
}

fn validate_batch_ocr_cohort(batch_mode: bool, total_count: usize, ocr_required_count: usize) -> Result<()> {
    if batch_mode && ocr_required_count > 0 && ocr_required_count < total_count {
        return Err(Error::Config(format!(
            "native batch benchmarks require a homogeneous OCR cohort, but the selected fixture cohort mixes {} \
             force-OCR and {} non-force-OCR fixture(s). Select a fixture directory/shard containing only one OCR \
             mode; the harness will not label sequential fallback as batch throughput.",
            ocr_required_count,
            total_count - ocr_required_count
        )));
    }
    Ok(())
}

fn load_quality_ground_truth(
    fixtures: &FixtureManager,
) -> Result<(HashMap<PathBuf, String>, HashMap<PathBuf, String>)> {
    let mut ground_truth_map = HashMap::new();
    let mut markdown_gt_map = HashMap::new();

    for (fixture_path, fixture) in fixtures.fixtures() {
        let fixture_dir = fixture_path.parent().unwrap_or_else(|| Path::new("."));
        let document_path = fixture.resolve_document_path(fixture_dir);
        let text_path = fixture.resolve_ground_truth_path(fixture_dir);
        let markdown_path = fixture.resolve_ground_truth_markdown_path(fixture_dir);

        if text_path.is_none() && markdown_path.is_none() {
            return Err(Error::Config(format!(
                "quality measurement requires text_file or markdown_file ground truth for {}",
                fixture.document.display()
            )));
        }

        let markdown = markdown_path
            .as_ref()
            .map(|path| {
                std::fs::read_to_string(path).map_err(|error| {
                    Error::Benchmark(format!(
                        "failed to read requested markdown ground truth for {}: {error}",
                        fixture.document.display()
                    ))
                })
            })
            .transpose()?;
        let text = if let Some(path) = text_path {
            std::fs::read_to_string(path).map_err(|error| {
                Error::Benchmark(format!(
                    "failed to read requested text ground truth for {}: {error}",
                    fixture.document.display()
                ))
            })?
        } else {
            markdown.clone().ok_or_else(|| {
                Error::Config(format!(
                    "quality measurement requires readable ground truth for {}",
                    fixture.document.display()
                ))
            })?
        };

        ground_truth_map.insert(document_path.clone(), text);
        if let Some(markdown) = markdown {
            markdown_gt_map.insert(document_path, markdown);
        }
    }

    Ok((ground_truth_map, markdown_gt_map))
}

/// Calculate statistics from iteration results
///
/// # Arguments
/// * `iterations` - Vector of iteration results to analyze
///
/// # Returns
/// Duration statistics including mean, median, std dev, and percentiles
fn calculate_statistics(iterations: &[IterationResult]) -> DurationStatistics {
    if iterations.is_empty() {
        return DurationStatistics {
            mean: Duration::from_secs(0),
            median: Duration::from_secs(0),
            std_dev_ms: 0.0,
            min: Duration::from_secs(0),
            max: Duration::from_secs(0),
            p95: Duration::from_secs(0),
            p99: Duration::from_secs(0),
            sample_count: 0,
        };
    }

    let durations: Vec<Duration> = iterations.iter().map(|i| i.duration).collect();

    let min = *durations.iter().min().unwrap_or(&Duration::from_secs(0));
    let max = *durations.iter().max().unwrap_or(&Duration::from_secs(0));

    let total_ms: f64 = durations.iter().map(|d| d.as_secs_f64() * 1000.0).sum();
    let mean_ms = total_ms / durations.len() as f64;
    let mean = Duration::from_secs_f64(mean_ms / 1000.0);

    let mut durations_ms: Vec<f64> = durations.iter().map(|d| d.as_secs_f64() * 1000.0).collect();
    durations_ms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let p50 = percentile_r7(&durations_ms, 0.50);
    let median = if p50.is_finite() {
        Duration::from_secs_f64(p50 / 1000.0)
    } else {
        Duration::from_secs(0)
    };

    let variance: f64 = if durations.len() > 1 {
        durations
            .iter()
            .map(|d| {
                let diff = d.as_secs_f64() * 1000.0 - mean_ms;
                diff * diff
            })
            .sum::<f64>()
            / (durations.len() - 1) as f64
    } else {
        0.0
    };

    let std_dev_ms = variance.sqrt();

    let p95_ms = percentile_r7(&durations_ms, 0.95);
    let p95 = if p95_ms.is_finite() {
        Duration::from_secs_f64(p95_ms / 1000.0)
    } else {
        Duration::from_secs(0)
    };

    let p99_ms = percentile_r7(&durations_ms, 0.99);
    let p99 = if p99_ms.is_finite() {
        Duration::from_secs_f64(p99_ms / 1000.0)
    } else {
        Duration::from_secs(0)
    };

    DurationStatistics {
        mean,
        median,
        std_dev_ms,
        min,
        max,
        p95,
        p99,
        sample_count: iterations.len(),
    }
}

/// Check if profiling is enabled via environment variable
///
/// # Returns
/// `true` if `ENABLE_PROFILING=true` is set, `false` otherwise
#[cfg(feature = "profiling")]
fn should_profile() -> bool {
    std::env::var("ENABLE_PROFILING").unwrap_or_default() == "true"
}

/// Aggregate performance metrics from iterations (average)
fn aggregate_metrics(iterations: &[IterationResult]) -> PerformanceMetrics {
    if iterations.is_empty() {
        return PerformanceMetrics::default();
    }

    let count = iterations.len() as f64;

    let baseline_memory_bytes = iterations
        .iter()
        .map(|i| i.metrics.baseline_memory_bytes)
        .max()
        .unwrap_or(0);

    let peak_memory_bytes = iterations
        .iter()
        .map(|i| i.metrics.peak_memory_bytes)
        .max()
        .unwrap_or(0);

    let peak_memory_delta_bytes = iterations
        .iter()
        .map(|i| i.metrics.peak_memory_delta_bytes)
        .max()
        .unwrap_or(0);

    let avg_cpu_percent = iterations.iter().map(|i| i.metrics.avg_cpu_percent).sum::<f64>() / count;

    let cpu_seconds = iterations.iter().map(|i| i.metrics.cpu_seconds).sum::<f64>() / count;

    let throughput_bytes_per_sec = iterations
        .iter()
        .map(|i| i.metrics.throughput_bytes_per_sec)
        .sum::<f64>()
        / count;

    let p50_memory_bytes = (iterations.iter().map(|i| i.metrics.p50_memory_bytes).sum::<u64>() as f64 / count) as u64;

    let p95_memory_bytes = (iterations.iter().map(|i| i.metrics.p95_memory_bytes).sum::<u64>() as f64 / count) as u64;

    let p99_memory_bytes = (iterations.iter().map(|i| i.metrics.p99_memory_bytes).sum::<u64>() as f64 / count) as u64;

    PerformanceMetrics {
        baseline_memory_bytes,
        peak_memory_bytes,
        peak_memory_delta_bytes,
        avg_cpu_percent,
        cpu_seconds,
        throughput_bytes_per_sec,
        p50_memory_bytes,
        p95_memory_bytes,
        p99_memory_bytes,
    }
}

fn average_durations(durations: impl Iterator<Item = Duration>) -> Option<Duration> {
    let (total, count) = durations.fold((Duration::ZERO, 0_u32), |(total, count), duration| {
        (total.saturating_add(duration), count.saturating_add(1))
    });
    (count > 0).then(|| total / count)
}

fn resolve_cohort_manifest_path(fixture_root: &Path, manifest_path: &Path) -> PathBuf {
    if manifest_path.is_absolute() || manifest_path.exists() {
        manifest_path.to_path_buf()
    } else {
        fixture_root.join(manifest_path)
    }
}

/// Orchestrates benchmark execution across fixtures and frameworks
pub struct BenchmarkRunner {
    config: BenchmarkConfig,
    registry: AdapterRegistry,
    fixtures: FixtureManager,
    cohort_manifest_path: Option<PathBuf>,
    cold_start_durations: HashMap<String, Duration>,
    framework_sizes: HashMap<String, DiskSizeInfo>,
    output_format: OutputFormat,
    fixed_batch_size: Option<usize>,
}

fn fixed_batch_ranges(item_count: usize, batch_size: Option<usize>) -> Result<Vec<std::ops::Range<usize>>> {
    let Some(batch_size) = batch_size else {
        return Ok((item_count > 0).then_some(0..item_count).into_iter().collect());
    };
    if batch_size == 0 {
        return Err(Error::Config("fixed batch size must be greater than zero".to_string()));
    }
    // Allow a smaller final batch. A framework benchmarks only the formats it declares support for,
    // so its eligible-document count need not be an exact multiple of the cohort's fixed batch size
    // (0 eligible → no batches). The last range is clamped to `item_count`.
    Ok((0..item_count)
        .step_by(batch_size)
        .map(|start| start..(start + batch_size).min(item_count))
        .collect())
}

/// Resolve the installation size to report for a benchmark result framework.
///
/// Competitors (liteparse, docling, ...) name their benchmark row identically to
/// their size-map key, so they resolve by a direct lookup (after stripping the
/// `-batch`/`-sync`/`-async` mode suffix).
///
/// Xberg benchmark rows are named `xberg-<format>-<pipeline>` (e.g.
/// `xberg-markdown-baseline`, `xberg-markdown-layout`) and are *not* size-map
/// keys, so they map onto the measured native `xberg-rust` footprint:
/// - heuristic pipelines (baseline/plaintext) ship without ML models, so they
///   report the shipped binary+dylibs only (`package_bytes`) — the fair
///   comparison against model-free tools like LiteParse;
/// - ML pipelines (layout/paddle/candle) additionally require the on-demand
///   model cache, so they report `package_bytes + model_bytes`.
fn resolve_installation_size(framework: &str, sizes: &HashMap<String, DiskSizeInfo>) -> Option<DiskSizeInfo> {
    let base_name = framework
        .trim_end_matches("-batch")
        .trim_end_matches("-sync")
        .trim_end_matches("-async");

    if let Some(size_info) = sizes.get(base_name) {
        return Some(size_info.clone());
    }

    if base_name.starts_with("xberg-") {
        let base = sizes.get("xberg-rust")?;
        let uses_models = ["layout", "paddle", "candle"].iter().any(|m| base_name.contains(m));
        let mut info = base.clone();
        if uses_models {
            info.size_bytes = base.package_bytes + base.model_bytes;
        } else {
            info.size_bytes = base.package_bytes;
            info.model_bytes = 0;
        }
        return Some(info);
    }

    None
}

impl BenchmarkRunner {
    fn select_frameworks(&self, framework_names: &[String]) -> Result<Vec<Arc<dyn FrameworkAdapter>>> {
        if framework_names.is_empty() {
            let mut names = self.registry.adapter_names();
            names.sort();
            return Ok(names
                .into_iter()
                .filter_map(|name| self.registry.get(&name))
                .filter(|adapter| adapter.supported_output_formats().contains(&self.output_format))
                .collect());
        }

        let mut selected = Vec::with_capacity(framework_names.len());
        for name in framework_names {
            let adapter = self
                .registry
                .get(name)
                .ok_or_else(|| Error::Config(format!("requested framework '{name}' is not registered")))?;
            if !adapter.supported_output_formats().contains(&self.output_format) {
                return Err(Error::Config(format!(
                    "framework '{name}' does not support {} output",
                    self.output_format
                )));
            }
            selected.push(adapter);
        }
        Ok(selected)
    }

    async fn setup_frameworks(frameworks: &[Arc<dyn FrameworkAdapter>]) -> Result<()> {
        let mut initialized = Vec::with_capacity(frameworks.len());
        for adapter in frameworks {
            if let Err(error) = adapter.setup().await {
                if let Err(teardown_error) = Self::teardown_frameworks(&initialized).await {
                    eprintln!("Warning: teardown after setup failure also failed: {teardown_error}");
                }
                return Err(error);
            }
            initialized.push(Arc::clone(adapter));
        }
        Ok(())
    }

    async fn teardown_frameworks(frameworks: &[Arc<dyn FrameworkAdapter>]) -> Result<()> {
        let mut first_error = None;
        for adapter in frameworks {
            if let Err(error) = adapter.teardown().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Create a new benchmark runner
    pub fn new(config: BenchmarkConfig, registry: AdapterRegistry) -> Self {
        Self::with_output_format(config, registry, OutputFormat::Markdown)
    }

    /// Create a new benchmark runner with a specific output format
    pub fn with_output_format(config: BenchmarkConfig, registry: AdapterRegistry, output_format: OutputFormat) -> Self {
        let framework_sizes = match crate::sizes::measure_framework_sizes() {
            Ok(sizes) => {
                if !sizes.is_empty() {
                    eprintln!("Measured disk sizes for {} frameworks", sizes.len());
                }
                sizes
                    .into_iter()
                    .map(|(name, fs)| {
                        (
                            name,
                            DiskSizeInfo {
                                size_bytes: fs.size_bytes,
                                package_bytes: fs.package_bytes,
                                system_deps_bytes: fs.system_deps_bytes,
                                model_bytes: fs.model_bytes,
                                method: fs.method,
                                description: fs.description,
                                system_deps_detail: fs.system_deps_detail,
                            },
                        )
                    })
                    .collect()
            }
            Err(e) => {
                eprintln!("Warning: Failed to measure framework sizes: {}", e);
                HashMap::new()
            }
        };

        Self {
            config,
            registry,
            fixtures: FixtureManager::new(),
            cohort_manifest_path: None,
            cold_start_durations: HashMap::new(),
            framework_sizes,
            output_format,
            fixed_batch_size: None,
        }
    }

    /// Load fixtures from a directory or file
    pub fn load_fixtures(&mut self, path: &PathBuf) -> Result<()> {
        self.cohort_manifest_path = None;
        if path.is_dir() {
            self.fixtures.load_fixtures_from_dir(path)?;
        } else {
            self.fixtures.load_fixture(path)?;
        }
        Ok(())
    }

    /// Load exactly the fixtures declared by an ordered cohort manifest.
    pub fn load_cohort(&mut self, fixture_root: &Path, manifest_path: &Path) -> Result<crate::CohortManifest> {
        self.cohort_manifest_path = None;
        let resolved_manifest_path = resolve_cohort_manifest_path(fixture_root, manifest_path);
        let manifest = crate::CohortManifest::from_file(&resolved_manifest_path).map_err(|error| {
            Error::Config(format!(
                "failed to load cohort manifest from resolved path '{}': {error}",
                resolved_manifest_path.display()
            ))
        })?;
        let fixtures = manifest
            .load_fixtures(fixture_root, &resolved_manifest_path)
            .map_err(|error| {
                Error::Config(format!(
                    "failed to load cohort fixtures using resolved manifest '{}': {error}",
                    resolved_manifest_path.display()
                ))
            })?;
        self.fixtures = fixtures;
        self.cohort_manifest_path = Some(resolved_manifest_path);
        Ok(manifest)
    }

    /// Require native batch invocations to use complete, fixed-size partitions.
    pub fn set_fixed_batch_size(&mut self, batch_size: usize) -> Result<()> {
        if batch_size == 0 {
            return Err(Error::Config("fixed batch size must be greater than zero".to_string()));
        }
        self.fixed_batch_size = Some(batch_size);
        Ok(())
    }

    /// Capture path-safe run provenance for the selected adapters and loaded corpus.
    pub fn capture_provenance(
        &self,
        framework_names: &[String],
        fixture_root: &Path,
        cohort: Option<&crate::CohortManifest>,
        cohort_manifest_path: Option<&Path>,
        fixed_batch_size: Option<usize>,
        models: &[crate::ModelProvenance],
    ) -> Result<crate::RunProvenance> {
        let frameworks = self.select_frameworks(framework_names)?;
        let resolved_cohort_manifest_path = match (cohort, cohort_manifest_path) {
            (None, None) => None,
            (Some(_), Some(_)) => Some(self.cohort_manifest_path.as_deref().ok_or_else(|| {
                Error::Config("cohort provenance requested without a successfully loaded cohort manifest".to_string())
            })?),
            _ => {
                return Err(Error::Config(
                    "cohort provenance requires both a manifest and its supplied path".to_string(),
                ));
            }
        };
        crate::RunProvenance::capture(crate::provenance::ProvenanceInputs {
            config: &self.config,
            output_format: self.output_format,
            fixture_root,
            fixtures: &self.fixtures,
            frameworks: &frameworks,
            cohort,
            cohort_manifest_path: resolved_cohort_manifest_path,
            fixed_batch_size,
            models,
        })
    }

    /// Retain only fixtures for the given shard (1-based index, total shards)
    pub fn apply_shard(&mut self, index: usize, total: usize) {
        self.fixtures.retain_shard(index, total);
    }

    /// Get count of loaded fixtures
    pub fn fixture_count(&self) -> usize {
        self.fixtures.len()
    }

    /// Enrich a benchmark result with framework size information
    ///
    /// # Arguments
    /// * `result` - Mutable reference to benchmark result to enrich
    fn enrich_with_framework_size(&self, result: &mut BenchmarkResult) {
        if let Some(size_info) = resolve_installation_size(&result.framework, &self.framework_sizes) {
            result.framework_capabilities.installation_size = Some(size_info);
        }
    }

    /// Run multiple iterations of a single extraction task (static method for async spawning)
    ///
    /// # Arguments
    /// * `file_path` - Path to file to extract
    /// * `adapter` - Framework adapter to use
    /// * `config` - Benchmark configuration
    /// * `cold_start_duration` - Optional cold start duration for this framework
    /// * `force_ocr` - When true, force OCR even if the document has a text layer
    /// * `output_format` - Output format for extraction
    ///
    /// # Returns
    /// Aggregated benchmark result with iterations and statistics
    async fn run_iterations_static(
        file_path: &Path,
        adapter: Arc<dyn FrameworkAdapter>,
        config: &BenchmarkConfig,
        cold_start_duration: Option<Duration>,
        force_ocr: bool,
        ocr_language: Option<&str>,
        output_format: OutputFormat,
    ) -> Result<BenchmarkResult> {
        let mut all_results = Vec::new();

        let estimated_task_duration_ms = if config.profiling.enabled {
            let warmup_start = std::time::Instant::now();
            let warmup_result = adapter
                .extract(file_path, config.timeout, force_ocr, ocr_language, output_format)
                .await?;
            let _warmup_duration = warmup_start.elapsed();
            warmup_result.duration.as_millis() as u64
        } else {
            config.profiling.task_duration_ms
        };

        #[cfg(feature = "profiling")]
        let sampling_frequency =
            crate::config::ProfilingConfig::calculate_optimal_frequency(estimated_task_duration_ms);

        #[cfg(feature = "profiling")]
        let profiler = if should_profile() && config.profiling.enabled {
            match ProfileGuard::new(sampling_frequency) {
                Ok(g) => {
                    eprintln!(
                        "Profiling enabled: {} Hz sampling frequency for ~{}ms tasks",
                        sampling_frequency, estimated_task_duration_ms
                    );
                    Some(g)
                }
                Err(e) => {
                    eprintln!("Warning: Failed to start profiler: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let warmup_start = if config.profiling.enabled { 1 } else { 0 };
        let mut warmup_timed_out = false;
        for _iteration in warmup_start..config.warmup_iterations {
            let result = adapter
                .extract(file_path, config.timeout, force_ocr, ocr_language, output_format)
                .await?;
            if result.error_kind == ErrorKind::Timeout {
                warmup_timed_out = true;
                break;
            }
            drop(result);
        }

        let amplification_factor = if config.profiling.enabled {
            calculate_amplified_iterations(estimated_task_duration_ms, 1000)
        } else {
            1
        };

        let effective_iterations = if warmup_timed_out {
            1
        } else {
            config.benchmark_iterations
        };
        'outer: for _iteration in 0..effective_iterations {
            for _amp in 0..amplification_factor {
                let result = adapter
                    .extract(file_path, config.timeout, force_ocr, ocr_language, output_format)
                    .await?;
                let timed_out = result.error_kind == ErrorKind::Timeout;
                all_results.push(result);
                if timed_out {
                    break 'outer;
                }
            }
        }

        #[cfg(feature = "profiling")]
        if let Some(profiler) = profiler {
            let framework_name = adapter.name();
            let mode_name = match config.benchmark_mode {
                BenchmarkMode::SingleFile => "single-file",
                BenchmarkMode::Batch => "batch",
            };
            let fixture_stem = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or_else(|| {
                eprintln!(
                    "Warning: Failed to extract valid UTF-8 filename from {:?}, using sanitized fallback",
                    file_path
                );
                "unknown_file"
            });

            let flamegraph_path = format!("flamegraphs/{}/{}/{}.svg", framework_name, mode_name, fixture_stem);
            let report_path = format!(
                "flamegraphs/{}/{}/{}_report.html",
                framework_name, mode_name, fixture_stem
            );

            match profiler.finish() {
                Ok(result) => {
                    eprintln!(
                        "Profiling complete: {} samples collected in {:?}",
                        result.sample_count, result.duration
                    );

                    if result.sample_count < config.profiling.sample_count_threshold {
                        eprintln!(
                            "Warning: Low sample count ({} < {} threshold); profile may have high variance",
                            result.sample_count, config.profiling.sample_count_threshold
                        );
                    }

                    if config.profiling.flamegraph_enabled {
                        let path = Path::new(&flamegraph_path);
                        if let Err(e) = result.generate_flamegraph(path) {
                            eprintln!("Warning: Failed to generate flamegraph: {}", e);
                        }

                        let profile_report = ProfileReport::from_profiling_result(&result, framework_name);
                        let html_report = profile_report.generate_html();

                        let report_file_path = Path::new(&report_path);
                        if let Some(parent) = report_file_path.parent()
                            && !parent.as_os_str().is_empty()
                        {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                eprintln!("Warning: Failed to create report directory: {}", e);
                            } else if let Err(e) = std::fs::write(report_file_path, html_report) {
                                eprintln!("Warning: Failed to write HTML report: {}", e);
                            } else {
                                eprintln!("Profile report written to: {}", report_path);
                            }
                        }
                    }
                }
                Err(e) => eprintln!("Warning: Profiling error: {}", e),
            }
        }

        if config.benchmark_iterations == 1 && !all_results.is_empty() {
            let mut result = all_results
                .into_iter()
                .next()
                .ok_or_else(|| Error::Benchmark("Failed to retrieve single iteration result".to_string()))?;
            result.cold_start_duration = cold_start_duration;
            result.system_load = Some(SystemLoad::capture());
            return Ok(result);
        }

        if all_results.is_empty() {
            return Err(Error::Benchmark("No successful iterations".to_string()));
        }

        let iterations: Vec<IterationResult> = all_results
            .iter()
            .enumerate()
            .map(|(idx, result)| IterationResult {
                iteration: idx + 1,
                duration: result.duration,
                extraction_duration: result.extraction_duration,
                metrics: result.metrics.clone(),
            })
            .collect();

        let statistics = calculate_statistics(&iterations);

        let aggregated_metrics = aggregate_metrics(&iterations);

        let extraction_durations: Vec<Duration> = all_results.iter().filter_map(|r| r.extraction_duration).collect();

        let avg_extraction_duration = if !extraction_durations.is_empty() {
            let total_ms: f64 = extraction_durations.iter().map(|d| d.as_secs_f64() * 1000.0).sum();
            let avg_ms = total_ms / extraction_durations.len() as f64;
            if avg_ms.is_finite() {
                Some(Duration::from_secs_f64(avg_ms / 1000.0))
            } else {
                None
            }
        } else {
            None
        };

        let subprocess_overhead = avg_extraction_duration.map(|ext| statistics.mean.saturating_sub(ext));

        let first_result = &all_results[0];
        let representative_result = all_results.iter().find(|result| result.success).unwrap_or(first_result);
        let all_success = all_results.iter().all(|result| result.success);
        let error_message = all_results
            .iter()
            .find(|result| !result.success)
            .and_then(|result| result.error_message.clone());

        let error_kind = if all_success {
            ErrorKind::None
        } else {
            all_results
                .iter()
                .filter(|result| !result.success)
                .map(|r| r.error_kind)
                .max_by_key(|ek| match ek {
                    ErrorKind::Timeout => 4,
                    ErrorKind::HarnessError => 3,
                    ErrorKind::ConfigSetupError => 2,
                    ErrorKind::FrameworkError => 1,
                    ErrorKind::EmptyContent => 1,
                    ErrorKind::None => 0,
                })
                .unwrap_or(ErrorKind::None)
        };

        let quality = representative_result.quality.clone();

        Ok(BenchmarkResult {
            framework: first_result.framework.clone(),
            output_format: first_result.output_format,
            file_path: first_result.file_path.clone(),
            file_size: first_result.file_size,
            success: all_success,
            error_message,
            error_kind,
            duration: statistics.mean,
            extraction_duration: avg_extraction_duration,
            subprocess_overhead,
            metrics: aggregated_metrics,
            quality,
            iterations,
            statistics: Some(statistics),
            cold_start_duration,
            file_extension: first_result.file_extension.clone(),
            framework_capabilities: first_result.framework_capabilities.clone(),
            pdf_metadata: representative_result.pdf_metadata.clone(),
            ocr_status: representative_result.ocr_status,
            extracted_text: representative_result.extracted_text.clone(),
            system_load: Some(SystemLoad::capture()),
        })
    }

    /// Run multiple iterations of batch extraction (static method for async spawning)
    ///
    /// # Arguments
    /// * `file_paths` - Paths to files to extract in batch
    /// * `adapter` - Framework adapter to use
    /// * `config` - Benchmark configuration
    /// * `cold_start_duration` - Optional cold start duration for this framework
    /// * `output_format` - Output format for extraction
    ///
    /// # Returns
    /// Vector of aggregated benchmark results (one per file) with iterations and statistics
    async fn run_batch_iterations_static(
        file_paths: Vec<PathBuf>,
        adapter: Arc<dyn FrameworkAdapter>,
        config: &BenchmarkConfig,
        cold_start_duration: Option<Duration>,
        force_ocr_flags: Vec<bool>,
        ocr_languages: Vec<Option<String>>,
        output_format: OutputFormat,
    ) -> Result<Vec<BenchmarkResult>> {
        let batch_capability = adapter.batch_capability().ok_or_else(|| {
            Error::Config(format!(
                "framework '{}' does not expose a verified native batch API",
                adapter.name()
            ))
        })?;
        if force_ocr_flags.len() != file_paths.len() {
            return Err(Error::Benchmark(format!(
                "batch force_ocr cardinality mismatch: received {} flags for {} files",
                force_ocr_flags.len(),
                file_paths.len()
            )));
        }
        if ocr_languages.len() != file_paths.len() {
            return Err(Error::Benchmark(format!(
                "batch ocr_languages cardinality mismatch: received {} values for {} files",
                ocr_languages.len(),
                file_paths.len()
            )));
        }

        let warmup_iterations = effective_batch_warmup_iterations(batch_capability, config.warmup_iterations);
        let total_iterations = warmup_iterations + config.benchmark_iterations;
        let mut all_batch_results = Vec::new();

        for iteration in 0..total_iterations {
            let refs: Vec<&std::path::Path> = file_paths.iter().map(|p| p.as_path()).collect();
            let mut batch_results = adapter
                .extract_batch(&refs, config.timeout, &force_ocr_flags, &ocr_languages, output_format)
                .await?;
            for result in &mut batch_results {
                result.framework_capabilities.batch_support = true;
                result.framework_capabilities.batch_capability = Some(batch_capability);
            }
            if batch_results.len() != file_paths.len() {
                return Err(Error::Benchmark(format!(
                    "framework '{}' returned {} batch results for {} files",
                    adapter.name(),
                    batch_results.len(),
                    file_paths.len()
                )));
            }

            if let Some(failed) = batch_results.iter().find(|result| !result.success) {
                return Err(Error::Benchmark(format!(
                    "framework '{}' returned a partial batch failure for {}: {}",
                    adapter.name(),
                    failed.file_path.display(),
                    failed
                        .error_message
                        .as_deref()
                        .unwrap_or("unspecified extraction failure")
                )));
            }

            let has_timeout = batch_results.iter().any(|r| r.error_kind == ErrorKind::Timeout);

            if iteration >= warmup_iterations || has_timeout {
                all_batch_results.push(batch_results);
            }

            if has_timeout {
                break;
            }
        }

        if config.benchmark_iterations == 1 && !all_batch_results.is_empty() {
            let mut result = all_batch_results
                .into_iter()
                .next()
                .ok_or_else(|| Error::Benchmark("Failed to retrieve single batch iteration result".to_string()))?;
            let system_load = Some(SystemLoad::capture());
            for r in &mut result {
                r.cold_start_duration = cold_start_duration;
                r.system_load = system_load;
            }
            return Ok(result);
        }

        if all_batch_results.is_empty() {
            return Err(Error::Benchmark("No batch results".to_string()));
        }

        let num_files = file_paths.len();
        let mut aggregated_results = Vec::new();

        for file_idx in 0..num_files {
            let mut file_iterations = Vec::new();
            for batch in &all_batch_results {
                file_iterations.push(&batch[file_idx]);
            }

            let iterations: Vec<IterationResult> = file_iterations
                .iter()
                .enumerate()
                .map(|(idx, result)| IterationResult {
                    iteration: idx + 1,
                    duration: result.duration,
                    extraction_duration: result.extraction_duration,
                    metrics: result.metrics.clone(),
                })
                .collect();

            let statistics = calculate_statistics(&iterations);
            let aggregated_metrics = aggregate_metrics(&iterations);

            let extraction_durations: Vec<Duration> =
                file_iterations.iter().filter_map(|r| r.extraction_duration).collect();

            let avg_extraction_duration = if !extraction_durations.is_empty() {
                let total_ms: f64 = extraction_durations.iter().map(|d| d.as_secs_f64() * 1000.0).sum();
                let avg_ms = total_ms / extraction_durations.len() as f64;
                if avg_ms.is_finite() {
                    Some(Duration::from_secs_f64(avg_ms / 1000.0))
                } else {
                    None
                }
            } else {
                None
            };

            let avg_subprocess_overhead =
                average_durations(file_iterations.iter().filter_map(|result| result.subprocess_overhead));

            let first_result = file_iterations[0];
            let representative_result = file_iterations
                .iter()
                .copied()
                .find(|result| result.success)
                .unwrap_or(first_result);
            let all_success = file_iterations.iter().all(|result| result.success);
            let error_message = file_iterations
                .iter()
                .find(|result| !result.success)
                .and_then(|result| result.error_message.clone());

            let error_kind = if all_success {
                ErrorKind::None
            } else {
                file_iterations
                    .iter()
                    .filter(|result| !result.success)
                    .map(|result| result.error_kind)
                    .max_by_key(|error_kind| match error_kind {
                        ErrorKind::Timeout => 4,
                        ErrorKind::HarnessError => 3,
                        ErrorKind::ConfigSetupError => 2,
                        ErrorKind::FrameworkError | ErrorKind::EmptyContent => 1,
                        ErrorKind::None => 0,
                    })
                    .unwrap_or(ErrorKind::None)
            };

            aggregated_results.push(BenchmarkResult {
                framework: first_result.framework.clone(),
                output_format: first_result.output_format,
                file_path: first_result.file_path.clone(),
                file_size: first_result.file_size,
                success: all_success,
                error_message,
                error_kind,
                duration: statistics.mean,
                extraction_duration: avg_extraction_duration,
                subprocess_overhead: avg_subprocess_overhead,
                metrics: aggregated_metrics,
                quality: representative_result.quality.clone(),
                iterations,
                statistics: Some(statistics),
                cold_start_duration,
                file_extension: first_result.file_extension.clone(),
                framework_capabilities: first_result.framework_capabilities.clone(),
                pdf_metadata: representative_result.pdf_metadata.clone(),
                ocr_status: representative_result.ocr_status,
                extracted_text: representative_result.extracted_text.clone(),
                system_load: Some(SystemLoad::capture()),
            });
        }

        Ok(aggregated_results)
    }

    /// Run benchmarks for specified frameworks
    ///
    /// # Arguments
    /// * `framework_names` - Names of frameworks to benchmark (empty = all registered)
    ///
    /// # Returns
    /// Vector of benchmark results
    pub async fn run(&mut self, framework_names: &[String]) -> Result<Vec<BenchmarkResult>> {
        let frameworks = self.select_frameworks(framework_names)?;

        if frameworks.is_empty() {
            return Err(Error::Benchmark("No frameworks available for benchmarking".to_string()));
        }

        let use_batch = matches!(self.config.benchmark_mode, BenchmarkMode::Batch);
        if use_batch && let Some(adapter) = frameworks.iter().find(|adapter| adapter.batch_capability().is_none()) {
            return Err(Error::Config(format!(
                "framework '{}' does not expose a verified native batch API",
                adapter.name()
            )));
        }

        let ocr_required_count = self
            .fixtures
            .fixtures()
            .iter()
            .filter(|(_, fixture)| fixture.requires_ocr())
            .count();
        validate_ocr_cohort(self.config.ocr_enabled, ocr_required_count)?;
        validate_batch_ocr_cohort(
            matches!(self.config.benchmark_mode, BenchmarkMode::Batch),
            self.fixtures.fixtures().len(),
            ocr_required_count,
        )?;

        let mut missing_files = Vec::new();
        for (fixture_path, fixture) in self.fixtures.fixtures() {
            let fixture_dir = fixture_path.parent().unwrap_or_else(|| std::path::Path::new("."));
            let document_path = fixture.resolve_document_path(fixture_dir);
            if !document_path.exists() {
                missing_files.push(document_path);
            }
        }
        if !missing_files.is_empty() {
            let samples: Vec<String> = missing_files
                .iter()
                .take(10)
                .map(|p| format!("  - {}", p.display()))
                .collect();
            return Err(Error::Benchmark(format!(
                "FATAL: {} fixture document(s) not found on disk. Benchmarks require all fixture files to exist.\nFirst {}:\n{}{}",
                missing_files.len(),
                samples.len(),
                samples.join("\n"),
                if missing_files.len() > 10 {
                    format!("\n  ... and {} more", missing_files.len() - 10)
                } else {
                    String::new()
                }
            )));
        }

        let quality_ground_truth = if self.config.measure_quality {
            Some(load_quality_ground_truth(&self.fixtures)?)
        } else {
            None
        };

        Self::setup_frameworks(&frameworks).await?;

        for adapter in &frameworks {
            if use_batch
                && adapter
                    .batch_capability()
                    .is_some_and(|capability| capability.timing_scope == BatchTimingScope::ColdEndToEndSubprocess)
            {
                println!(
                    "Skipping warmup for {}: each batch invocation is measured cold end-to-end",
                    adapter.name()
                );
                continue;
            }
            let warmup_fixture = self
                .fixtures
                .fixtures()
                .iter()
                .find(|(_, fixture)| adapter.supports_format(&fixture.file_type));

            if let Some((fixture_path, fixture)) = warmup_fixture {
                let fixture_dir = fixture_path.parent().unwrap_or_else(|| std::path::Path::new("."));
                let warmup_file = fixture.resolve_document_path(fixture_dir);

                println!("Warming up {} with {}...", adapter.name(), warmup_file.display());
                match adapter
                    .warmup(&warmup_file, self.config.timeout, self.output_format)
                    .await
                {
                    Ok(cold_start) => {
                        println!("  Cold start: {:?}", cold_start);
                        self.cold_start_durations.insert(adapter.name().to_string(), cold_start);
                    }
                    Err(warmup_error) => {
                        let teardown_error = Self::teardown_frameworks(&frameworks).await.err();
                        let teardown_context = teardown_error
                            .map(|error| format!("; teardown also failed: {error}"))
                            .unwrap_or_default();
                        return Err(Error::Benchmark(format!(
                            "warmup failed for '{}': {}{}",
                            adapter.name(),
                            warmup_error,
                            teardown_context
                        )));
                    }
                }
            } else {
                eprintln!(
                    "  Warning: No compatible fixture found for warmup of {}",
                    adapter.name()
                );
            }
        }

        let mut results = Vec::new();

        if use_batch {
            use std::collections::HashMap;

            let mut adapter_files: HashMap<String, Vec<(PathBuf, bool, Option<String>)>> = HashMap::new();

            for (fixture_path, fixture) in self.fixtures.fixtures() {
                let force_ocr = fixture.requires_ocr();
                for adapter in &frameworks {
                    if !adapter.supports_format(&fixture.file_type) {
                        continue;
                    }
                    if let Some(name) = fixture.document.file_name().and_then(|n| n.to_str())
                        && adapter.should_skip_file(name)
                    {
                        continue;
                    }

                    let fixture_dir = fixture_path.parent().unwrap_or_else(|| std::path::Path::new("."));
                    let document_path = fixture.resolve_document_path(fixture_dir);

                    adapter_files.entry(adapter.name().to_string()).or_default().push((
                        document_path,
                        force_ocr,
                        fixture.ocr_language().map(str::to_string),
                    ));
                }
            }

            let config = self.config.clone();

            for adapter in &frameworks {
                let adapter_name = adapter.name();

                if let Some(entries) = adapter_files.get(adapter_name) {
                    if entries.is_empty() {
                        continue;
                    }

                    let ranges = fixed_batch_ranges(entries.len(), self.fixed_batch_size).map_err(|error| {
                        Error::Config(format!(
                            "framework '{adapter_name}' fixed batch validation failed: {error}"
                        ))
                    })?;

                    for range in ranges {
                        let mut file_paths = Vec::new();
                        let mut force_ocr_flags = Vec::new();
                        let mut ocr_languages = Vec::new();
                        for (path, force_ocr, ocr_language) in &entries[range] {
                            file_paths.push(path.clone());
                            force_ocr_flags.push(*force_ocr);
                            ocr_languages.push(ocr_language.clone());
                        }
                        let adapter = Arc::clone(adapter);
                        let config = config.clone();
                        let cold_start = self.cold_start_durations.get(adapter_name).copied();

                        match Self::run_batch_iterations_static(
                            file_paths,
                            adapter,
                            &config,
                            cold_start,
                            force_ocr_flags,
                            ocr_languages,
                            self.output_format,
                        )
                        .await
                        {
                            Ok(mut batch_results) => {
                                for result in &mut batch_results {
                                    self.enrich_with_framework_size(result);
                                }
                                results.extend(batch_results);
                            }
                            Err(e) => {
                                if let Err(teardown_error) = Self::teardown_frameworks(&frameworks).await {
                                    eprintln!("Warning: teardown after batch failure also failed: {teardown_error}");
                                }
                                return Err(e);
                            }
                        }
                    }
                }
            }
        } else {
            let mut task_queue: Vec<SingleBenchmarkTask> = Vec::new();

            for (fixture_path, fixture) in self.fixtures.fixtures() {
                let force_ocr = fixture.requires_ocr();
                for adapter in &frameworks {
                    if !adapter.supports_format(&fixture.file_type) {
                        continue;
                    }
                    if let Some(name) = fixture.document.file_name().and_then(|n| n.to_str())
                        && adapter.should_skip_file(name)
                    {
                        continue;
                    }

                    let fixture_dir = fixture_path.parent().unwrap_or_else(|| std::path::Path::new("."));
                    let document_path = fixture.resolve_document_path(fixture_dir);

                    task_queue.push((
                        document_path,
                        adapter.name().to_string(),
                        Arc::clone(adapter),
                        force_ocr,
                        fixture.ocr_language().map(str::to_string),
                    ));
                }
            }

            let config = self.config.clone();

            for (file_path, framework_name, adapter, force_ocr, ocr_language) in task_queue {
                let cold_start = self.cold_start_durations.get(&framework_name).copied();
                match Self::run_iterations_static(
                    &file_path,
                    adapter,
                    &config,
                    cold_start,
                    force_ocr,
                    ocr_language.as_deref(),
                    self.output_format,
                )
                .await
                {
                    Ok(mut result) => {
                        self.enrich_with_framework_size(&mut result);
                        results.push(result);
                    }
                    Err(task_error) => {
                        // A missing row would make downstream coverage look better than the
                        // eligible corpus actually was, so task-level harness errors abort the
                        // run instead of being silently omitted. ~keep
                        let teardown_error = Self::teardown_frameworks(&frameworks).await.err();
                        let teardown_context = teardown_error
                            .map(|error| format!("; teardown also failed: {error}"))
                            .unwrap_or_default();
                        return Err(Error::Benchmark(format!(
                            "benchmark task failed for '{framework_name}' on {}: {task_error}{teardown_context}",
                            file_path.display()
                        )));
                    }
                }
            }
        }

        if let Some((ground_truth_map, markdown_gt_map)) = quality_ground_truth {
            for result in &mut results {
                if let Some(ref extracted) = result.extracted_text
                    && let Some(gt_text) = ground_truth_map.get(&result.file_path)
                {
                    let md_gt = markdown_gt_map.get(&result.file_path).map(|s| s.as_str());
                    result.quality = Some(crate::quality::compute_quality_with_structure(
                        extracted,
                        gt_text,
                        md_gt,
                        self.output_format,
                    ));
                }
            }
        }

        Self::teardown_frameworks(&frameworks).await?;

        Ok(results)
    }

    /// Get reference to benchmark configuration
    pub fn config(&self) -> &BenchmarkConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Fixture;
    use crate::types::{FrameworkCapabilities, OcrStatus};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn fixed_batch_ranges_are_complete_and_ordered() {
        assert_eq!(fixed_batch_ranges(8, Some(4)).unwrap(), [0..4, 4..8]);
        let legacy_batch = fixed_batch_ranges(3, None).unwrap();
        assert_eq!(legacy_batch.len(), 1);
        assert_eq!(legacy_batch[0], 0..3);
    }

    #[test]
    fn fixed_batch_ranges_allow_smaller_final_batch() {
        // A non-multiple eligible count yields a smaller final batch rather than an error.
        assert_eq!(fixed_batch_ranges(5, Some(4)).unwrap(), [0..4, 4..5]);
        let single = fixed_batch_ranges(1, Some(4)).unwrap();
        assert_eq!(single.len(), 1);
        assert_eq!(single[0], 0..1);
        // Zero documents -> no batches; a zero batch size is still rejected.
        assert!(fixed_batch_ranges(0, Some(4)).unwrap().is_empty());
        assert!(fixed_batch_ranges(0, Some(0)).is_err());
    }

    #[test]
    fn repeated_batch_overhead_is_averaged() {
        let overheads = [Duration::from_millis(5), Duration::from_millis(15)];

        assert_eq!(
            average_durations(overheads.into_iter()),
            Some(Duration::from_millis(10))
        );
        assert_eq!(average_durations(std::iter::empty()), None);
    }

    struct SequenceAdapter {
        calls: AtomicUsize,
    }

    struct TeardownAdapter {
        name: &'static str,
        setup_calls: Arc<AtomicUsize>,
        teardown_calls: Arc<AtomicUsize>,
        fail_setup: bool,
        fail_teardown: bool,
    }

    struct FailedWarmupAdapter {
        teardown_calls: Arc<AtomicUsize>,
    }

    struct TaskErrorAdapter {
        calls: AtomicUsize,
    }

    struct RecordingBatchAdapter {
        batches: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
    }

    impl RecordingBatchAdapter {
        fn success(file_path: &Path, output_format: OutputFormat) -> BenchmarkResult {
            BenchmarkResult {
                framework: "recording".to_string(),
                output_format,
                file_path: file_path.to_path_buf(),
                file_size: 1,
                success: true,
                error_message: None,
                error_kind: ErrorKind::None,
                duration: Duration::from_millis(1),
                extraction_duration: None,
                subprocess_overhead: None,
                metrics: PerformanceMetrics::default(),
                quality: None,
                iterations: vec![],
                statistics: None,
                cold_start_duration: None,
                file_extension: "pdf".to_string(),
                framework_capabilities: FrameworkCapabilities::default(),
                pdf_metadata: None,
                ocr_status: OcrStatus::NotUsed,
                extracted_text: Some("ok".to_string()),
                system_load: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl FrameworkAdapter for RecordingBatchAdapter {
        fn name(&self) -> &str {
            "recording"
        }

        fn supports_format(&self, file_type: &str) -> bool {
            file_type == "pdf"
        }

        fn supported_output_formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::Markdown]
        }

        async fn extract(
            &self,
            file_path: &Path,
            _timeout: Duration,
            _force_ocr: bool,
            _ocr_language: Option<&str>,
            output_format: OutputFormat,
        ) -> Result<BenchmarkResult> {
            Ok(Self::success(file_path, output_format))
        }

        async fn extract_batch(
            &self,
            file_paths: &[&Path],
            _timeout: Duration,
            _force_ocr: &[bool],
            _ocr_languages: &[Option<String>],
            output_format: OutputFormat,
        ) -> Result<Vec<BenchmarkResult>> {
            self.batches.lock().unwrap().push(
                file_paths
                    .iter()
                    .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                    .collect(),
            );
            Ok(file_paths
                .iter()
                .map(|path| Self::success(path, output_format))
                .collect())
        }

        fn batch_capability(&self) -> Option<BatchCapability> {
            Some(BatchCapability {
                entry_point: crate::types::BatchEntryPoint::XbergCliExtractBatch,
                timing_scope: BatchTimingScope::WarmSteadyState,
                per_item_timing: false,
            })
        }
    }

    impl SequenceAdapter {
        fn result(&self, file_path: &Path, output_format: OutputFormat) -> BenchmarkResult {
            let success = self.calls.fetch_add(1, Ordering::SeqCst) % 2 == 1;
            BenchmarkResult {
                framework: "sequence".to_string(),
                output_format,
                file_path: file_path.to_path_buf(),
                file_size: 10,
                success,
                error_message: (!success).then(|| "measured iteration failed".to_string()),
                error_kind: if success {
                    ErrorKind::None
                } else {
                    ErrorKind::FrameworkError
                },
                duration: Duration::from_millis(10),
                extraction_duration: Some(Duration::from_millis(5)),
                subprocess_overhead: Some(Duration::from_millis(5)),
                metrics: PerformanceMetrics::default(),
                quality: None,
                iterations: vec![],
                statistics: None,
                cold_start_duration: None,
                file_extension: "pdf".to_string(),
                framework_capabilities: FrameworkCapabilities::default(),
                pdf_metadata: None,
                ocr_status: OcrStatus::Unknown,
                extracted_text: success.then(|| "successful payload".to_string()),
                system_load: None,
            }
        }
    }

    #[async_trait::async_trait]
    impl FrameworkAdapter for SequenceAdapter {
        fn name(&self) -> &str {
            "sequence"
        }

        fn supports_format(&self, _file_type: &str) -> bool {
            true
        }

        fn supported_output_formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::Markdown]
        }

        async fn extract(
            &self,
            file_path: &Path,
            _timeout: Duration,
            _force_ocr: bool,
            _ocr_language: Option<&str>,
            output_format: OutputFormat,
        ) -> Result<BenchmarkResult> {
            Ok(self.result(file_path, output_format))
        }

        async fn extract_batch(
            &self,
            file_paths: &[&Path],
            _timeout: Duration,
            _force_ocr: &[bool],
            _ocr_languages: &[Option<String>],
            output_format: OutputFormat,
        ) -> Result<Vec<BenchmarkResult>> {
            Ok(file_paths.iter().map(|path| self.result(path, output_format)).collect())
        }

        fn batch_capability(&self) -> Option<crate::types::BatchCapability> {
            Some(crate::types::BatchCapability {
                entry_point: crate::types::BatchEntryPoint::XbergCliExtractBatch,
                timing_scope: crate::types::BatchTimingScope::WarmSteadyState,
                per_item_timing: false,
            })
        }
    }

    #[async_trait::async_trait]
    impl FrameworkAdapter for TeardownAdapter {
        fn name(&self) -> &str {
            self.name
        }

        fn supports_format(&self, _file_type: &str) -> bool {
            true
        }

        fn supported_output_formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::Markdown]
        }

        async fn extract(
            &self,
            _file_path: &Path,
            _timeout: Duration,
            _force_ocr: bool,
            _ocr_language: Option<&str>,
            _output_format: OutputFormat,
        ) -> Result<BenchmarkResult> {
            Err(Error::Benchmark("unused test extraction".to_string()))
        }

        async fn setup(&self) -> Result<()> {
            self.setup_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_setup {
                Err(Error::Benchmark(format!("{} setup failed", self.name)))
            } else {
                Ok(())
            }
        }

        async fn teardown(&self) -> Result<()> {
            self.teardown_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_teardown {
                Err(Error::Benchmark(format!("{} teardown failed", self.name)))
            } else {
                Ok(())
            }
        }
    }

    #[async_trait::async_trait]
    impl FrameworkAdapter for FailedWarmupAdapter {
        fn name(&self) -> &str {
            "failed-warmup"
        }

        fn supports_format(&self, file_type: &str) -> bool {
            file_type == "pdf"
        }

        fn supported_output_formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::Markdown]
        }

        async fn extract(
            &self,
            file_path: &Path,
            _timeout: Duration,
            _force_ocr: bool,
            _ocr_language: Option<&str>,
            output_format: OutputFormat,
        ) -> Result<BenchmarkResult> {
            Ok(BenchmarkResult {
                framework: self.name().to_string(),
                output_format,
                file_path: file_path.to_path_buf(),
                file_size: 1,
                success: false,
                error_message: Some("intentional warmup failure".to_string()),
                error_kind: ErrorKind::FrameworkError,
                duration: Duration::from_millis(1),
                extraction_duration: None,
                subprocess_overhead: None,
                metrics: PerformanceMetrics::default(),
                quality: None,
                iterations: vec![],
                statistics: None,
                cold_start_duration: None,
                file_extension: "pdf".to_string(),
                framework_capabilities: FrameworkCapabilities::default(),
                pdf_metadata: None,
                ocr_status: OcrStatus::Unknown,
                extracted_text: None,
                system_load: None,
            })
        }

        async fn teardown(&self) -> Result<()> {
            self.teardown_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[async_trait::async_trait]
    impl FrameworkAdapter for TaskErrorAdapter {
        fn name(&self) -> &str {
            "task-error"
        }

        fn supports_format(&self, file_type: &str) -> bool {
            file_type == "pdf"
        }

        fn supported_output_formats(&self) -> Vec<OutputFormat> {
            vec![OutputFormat::Markdown]
        }

        async fn extract(
            &self,
            file_path: &Path,
            _timeout: Duration,
            _force_ocr: bool,
            _ocr_language: Option<&str>,
            output_format: OutputFormat,
        ) -> Result<BenchmarkResult> {
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                Ok(RecordingBatchAdapter::success(file_path, output_format))
            } else {
                Err(Error::Benchmark("intentional task error".to_string()))
            }
        }
    }

    fn disk_size(size: u64, package: u64, model: u64) -> DiskSizeInfo {
        DiskSizeInfo {
            size_bytes: size,
            package_bytes: package,
            system_deps_bytes: 0,
            model_bytes: model,
            method: "binary_size".to_string(),
            description: "test".to_string(),
            system_deps_detail: HashMap::new(),
        }
    }

    #[test]
    fn ocr_required_fixture_cohort_is_never_silently_skipped() {
        assert!(validate_ocr_cohort(false, 0).is_ok());
        assert!(validate_ocr_cohort(true, 2).is_ok());
        let error = validate_ocr_cohort(false, 2).unwrap_err();
        assert!(error.to_string().contains("--ocr"));
        assert!(error.to_string().contains("will not silently omit"));
    }

    #[test]
    fn native_batch_requires_homogeneous_ocr_cohort() {
        assert!(validate_batch_ocr_cohort(false, 2, 1).is_ok());
        assert!(validate_batch_ocr_cohort(true, 2, 0).is_ok());
        assert!(validate_batch_ocr_cohort(true, 2, 2).is_ok());
        let error = validate_batch_ocr_cohort(true, 3, 1).unwrap_err();
        assert!(error.to_string().contains("homogeneous OCR cohort"));
        assert!(error.to_string().contains("will not label sequential fallback"));
    }

    #[test]
    fn cold_subprocess_batch_never_runs_discarded_warmup_iterations() {
        let cold = BatchCapability {
            entry_point: crate::types::BatchEntryPoint::DoclingJobkit,
            timing_scope: BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing: false,
        };
        let warm = BatchCapability {
            entry_point: crate::types::BatchEntryPoint::XbergCliExtractBatch,
            timing_scope: BatchTimingScope::WarmSteadyState,
            per_item_timing: true,
        };

        assert_eq!(effective_batch_warmup_iterations(cold, 3), 0);
        assert_eq!(effective_batch_warmup_iterations(warm, 3), 3);
    }

    #[tokio::test]
    async fn teardown_attempts_every_framework_after_an_error() {
        let setup_calls = Arc::new(AtomicUsize::new(0));
        let first_calls = Arc::new(AtomicUsize::new(0));
        let second_calls = Arc::new(AtomicUsize::new(0));
        let frameworks: Vec<Arc<dyn FrameworkAdapter>> = vec![
            Arc::new(TeardownAdapter {
                name: "first",
                setup_calls: Arc::clone(&setup_calls),
                teardown_calls: Arc::clone(&first_calls),
                fail_setup: false,
                fail_teardown: true,
            }),
            Arc::new(TeardownAdapter {
                name: "second",
                setup_calls,
                teardown_calls: Arc::clone(&second_calls),
                fail_setup: false,
                fail_teardown: false,
            }),
        ];

        let error = BenchmarkRunner::teardown_frameworks(&frameworks).await.unwrap_err();

        assert!(error.to_string().contains("first teardown failed"));
        assert_eq!(first_calls.load(Ordering::SeqCst), 1);
        assert_eq!(second_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn setup_failure_tears_down_only_previously_initialized_frameworks() {
        let first_setup = Arc::new(AtomicUsize::new(0));
        let first_teardown = Arc::new(AtomicUsize::new(0));
        let second_setup = Arc::new(AtomicUsize::new(0));
        let second_teardown = Arc::new(AtomicUsize::new(0));
        let frameworks: Vec<Arc<dyn FrameworkAdapter>> = vec![
            Arc::new(TeardownAdapter {
                name: "first",
                setup_calls: Arc::clone(&first_setup),
                teardown_calls: Arc::clone(&first_teardown),
                fail_setup: false,
                fail_teardown: false,
            }),
            Arc::new(TeardownAdapter {
                name: "second",
                setup_calls: Arc::clone(&second_setup),
                teardown_calls: Arc::clone(&second_teardown),
                fail_setup: true,
                fail_teardown: false,
            }),
        ];

        let error = BenchmarkRunner::setup_frameworks(&frameworks).await.unwrap_err();

        assert!(error.to_string().contains("second setup failed"));
        assert_eq!(first_setup.load(Ordering::SeqCst), 1);
        assert_eq!(second_setup.load(Ordering::SeqCst), 1);
        assert_eq!(first_teardown.load(Ordering::SeqCst), 1);
        assert_eq!(second_teardown.load(Ordering::SeqCst), 0);
    }

    fn xberg_size_map() -> HashMap<String, DiskSizeInfo> {
        let mut sizes = HashMap::new();
        // shipped binary+dylibs = 40 MB, on-demand model cache = 525 MB. ~keep
        sizes.insert("xberg-rust".to_string(), disk_size(565, 40, 525));
        sizes.insert("liteparse".to_string(), disk_size(35, 35, 0));
        sizes
    }

    #[test]
    fn should_resolve_competitor_size_by_direct_name() {
        let sizes = xberg_size_map();
        let info = resolve_installation_size("liteparse", &sizes).unwrap();
        assert_eq!(info.size_bytes, 35);
    }

    #[test]
    fn should_strip_batch_suffix_for_competitor_lookup() {
        let sizes = xberg_size_map();
        let info = resolve_installation_size("liteparse-batch", &sizes).unwrap();
        assert_eq!(info.size_bytes, 35);
    }

    #[test]
    fn should_report_shipped_only_for_xberg_heuristic_rows() {
        let sizes = xberg_size_map();
        // Baseline/plaintext heuristic pipelines ship without ML models. ~keep
        for name in [
            "xberg-markdown-baseline",
            "xberg-plaintext-baseline",
            "xberg-markdown-baseline-batch",
        ] {
            let info = resolve_installation_size(name, &sizes).unwrap();
            assert_eq!(info.size_bytes, 40, "{name} should report shipped-only size");
            assert_eq!(info.model_bytes, 0, "{name} should not count model cache");
        }
    }

    #[test]
    fn should_include_models_for_xberg_ml_rows() {
        let sizes = xberg_size_map();
        for name in [
            "xberg-markdown-layout",
            "xberg-markdown-layout-batch",
            "xberg-markdown-paddle-ocr",
        ] {
            let info = resolve_installation_size(name, &sizes).unwrap();
            assert_eq!(info.size_bytes, 565, "{name} should include model cache");
            assert_eq!(info.model_bytes, 525);
        }
    }

    #[test]
    fn should_return_none_when_xberg_rust_unmeasured() {
        let sizes = HashMap::new();
        assert!(resolve_installation_size("xberg-markdown-baseline", &sizes).is_none());
        assert!(resolve_installation_size("unknown-framework", &sizes).is_none());
    }

    #[tokio::test]
    async fn test_benchmark_runner_creation() {
        let config = BenchmarkConfig::default();
        let registry = AdapterRegistry::new();
        let runner = BenchmarkRunner::new(config, registry);

        assert_eq!(runner.fixture_count(), 0);
    }

    fn write_ordered_cohort(temp: &Path, file_types: &[&str]) -> PathBuf {
        let fixtures = ["d.json", "b.json", "a.json", "c.json"];
        for (fixture_name, file_type) in fixtures.iter().zip(file_types) {
            let document_name = fixture_name.replace(".json", ".pdf");
            std::fs::write(temp.join(&document_name), b"x").unwrap();
            std::fs::write(
                temp.join(fixture_name),
                serde_json::json!({
                    "document": document_name,
                    "file_type": file_type,
                    "file_size": 1
                })
                .to_string(),
            )
            .unwrap();
        }
        let manifest = temp.join("cohort.json");
        std::fs::write(
            &manifest,
            serde_json::json!({
                "schema_version": 1,
                "name": "ordered",
                "batch_size": 2,
                "fixtures": fixtures
            })
            .to_string(),
        )
        .unwrap();
        manifest
    }

    #[test]
    fn cohort_manifest_path_preserves_absolute_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cohort.json");

        assert_eq!(resolve_cohort_manifest_path(Path::new("fixtures"), &path), path);
    }

    #[test]
    fn cohort_manifest_path_preserves_existing_cwd_relative_path() {
        let current_dir = std::env::current_dir().unwrap();
        let cwd_temp = tempfile::tempdir_in(&current_dir).unwrap();
        let fixture_root = tempfile::tempdir().unwrap();
        let manifest = cwd_temp.path().join("cohort.json");
        std::fs::write(&manifest, "cwd").unwrap();
        let relative = manifest.strip_prefix(&current_dir).unwrap();
        let fixture_relative = fixture_root.path().join(relative);
        std::fs::create_dir_all(fixture_relative.parent().unwrap()).unwrap();
        std::fs::write(fixture_relative, "fixture root").unwrap();

        assert_eq!(resolve_cohort_manifest_path(fixture_root.path(), relative), relative);
    }

    #[test]
    fn provenance_uses_loaded_fixture_relative_manifest_after_cwd_collision() {
        let current_dir = std::env::current_dir().unwrap();
        let cwd_temp = tempfile::tempdir_in(&current_dir).unwrap();
        let fixture_root = tempfile::tempdir().unwrap();
        let original_manifest = write_ordered_cohort(fixture_root.path(), &["pdf", "pdf", "pdf", "pdf"]);
        let relative_manifest = cwd_temp.path().strip_prefix(&current_dir).unwrap().join("ordered.json");
        let nested_manifest = fixture_root.path().join(&relative_manifest);
        std::fs::create_dir_all(nested_manifest.parent().unwrap()).unwrap();
        std::fs::rename(original_manifest, &nested_manifest).unwrap();
        assert!(!relative_manifest.exists());

        let mut runner = BenchmarkRunner::new(BenchmarkConfig::default(), AdapterRegistry::new());
        let manifest = runner.load_cohort(fixture_root.path(), &relative_manifest).unwrap();
        std::fs::write(&relative_manifest, "cwd collision created after cohort load").unwrap();
        let provenance = runner
            .capture_provenance(
                &[],
                fixture_root.path(),
                Some(&manifest),
                Some(&relative_manifest),
                Some(manifest.batch_size),
                &[],
            )
            .unwrap();
        let fixture_digest = blake3::hash(&std::fs::read(&nested_manifest).unwrap())
            .to_hex()
            .to_string();
        let cwd_digest = blake3::hash(&std::fs::read(&relative_manifest).unwrap())
            .to_hex()
            .to_string();

        assert_eq!(manifest.name, "ordered");
        assert_eq!(runner.fixture_count(), 4);
        assert_eq!(
            provenance.corpus.cohort_manifest_blake3.as_deref(),
            Some(fixture_digest.as_str())
        );
        assert_ne!(fixture_digest, cwd_digest);
    }

    #[test]
    fn failed_cohort_load_clears_path_and_names_resolved_path() {
        let current_dir = std::env::current_dir().unwrap();
        let cwd_temp = tempfile::tempdir_in(&current_dir).unwrap();
        let fixture_root = tempfile::tempdir().unwrap();
        let manifest = write_ordered_cohort(fixture_root.path(), &["pdf", "pdf", "pdf", "pdf"]);
        let missing = cwd_temp.path().strip_prefix(&current_dir).unwrap().join("missing.json");
        assert!(!missing.exists());
        let resolved_missing = fixture_root.path().join(&missing);
        let mut runner = BenchmarkRunner::new(BenchmarkConfig::default(), AdapterRegistry::new());
        runner.load_cohort(fixture_root.path(), &manifest).unwrap();
        assert!(runner.cohort_manifest_path.is_some());

        let error = runner.load_cohort(fixture_root.path(), &missing).unwrap_err();

        assert!(runner.cohort_manifest_path.is_none());
        assert!(error.to_string().contains(&resolved_missing.display().to_string()));

        let invalid_fixture_manifest = fixture_root.path().join("missing-fixture.json");
        std::fs::write(
            &invalid_fixture_manifest,
            serde_json::json!({
                "schema_version": 1,
                "name": "missing-fixture",
                "batch_size": 1,
                "fixtures": ["absent.json"]
            })
            .to_string(),
        )
        .unwrap();
        let error = runner
            .load_cohort(fixture_root.path(), &invalid_fixture_manifest)
            .unwrap_err();

        assert!(runner.cohort_manifest_path.is_none());
        assert!(
            error
                .to_string()
                .contains(&invalid_fixture_manifest.display().to_string())
        );
    }

    #[test]
    fn loading_regular_fixtures_clears_cohort_path() {
        let fixture_root = tempfile::tempdir().unwrap();
        let manifest = write_ordered_cohort(fixture_root.path(), &["pdf", "pdf", "pdf", "pdf"]);
        let mut runner = BenchmarkRunner::new(BenchmarkConfig::default(), AdapterRegistry::new());
        runner.load_cohort(fixture_root.path(), &manifest).unwrap();
        assert!(runner.cohort_manifest_path.is_some());

        runner.load_fixtures(&fixture_root.path().join("d.json")).unwrap();

        assert!(runner.cohort_manifest_path.is_none());
    }

    #[tokio::test]
    async fn fixed_batches_preserve_manifest_order_across_native_calls() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = write_ordered_cohort(temp.path(), &["pdf", "pdf", "pdf", "pdf"]);
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut registry = AdapterRegistry::new();
        registry
            .register(Arc::new(RecordingBatchAdapter {
                batches: Arc::clone(&batches),
            }))
            .unwrap();
        let config = BenchmarkConfig {
            benchmark_mode: BenchmarkMode::Batch,
            warmup_iterations: 0,
            benchmark_iterations: 1,
            ..Default::default()
        };
        let mut runner = BenchmarkRunner::new(config, registry);
        runner.load_cohort(temp.path(), &manifest).unwrap();
        runner.set_fixed_batch_size(2).unwrap();

        runner.run(&["recording".to_string()]).await.unwrap();

        assert_eq!(
            *batches.lock().unwrap(),
            [
                vec!["d.pdf".to_string(), "b.pdf".to_string()],
                vec!["a.pdf".to_string(), "c.pdf".to_string()]
            ]
        );
    }

    #[tokio::test]
    async fn fixed_batches_allow_partial_after_adapter_filter() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = write_ordered_cohort(temp.path(), &["pdf", "txt", "pdf", "pdf"]);
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut registry = AdapterRegistry::new();
        registry
            .register(Arc::new(RecordingBatchAdapter {
                batches: Arc::clone(&batches),
            }))
            .unwrap();
        let config = BenchmarkConfig {
            benchmark_mode: BenchmarkMode::Batch,
            warmup_iterations: 0,
            benchmark_iterations: 1,
            ..Default::default()
        };
        let mut runner = BenchmarkRunner::new(config, registry);
        runner.load_cohort(temp.path(), &manifest).unwrap();
        runner.set_fixed_batch_size(2).unwrap();

        runner.run(&["recording".to_string()]).await.unwrap();

        // The adapter supports only pdf, so the txt fixture (b.pdf) is filtered out before the
        // native batch call. The 3 eligible pdfs partition into a full batch of 2 and a smaller
        // final batch of 1 (manifest fixture order d, a, c) rather than aborting the run.
        assert_eq!(
            *batches.lock().unwrap(),
            [
                vec!["d.pdf".to_string(), "a.pdf".to_string()],
                vec!["c.pdf".to_string()],
            ]
        );
    }

    #[tokio::test]
    async fn repeated_native_batch_preserves_one_sample_per_document_and_iteration() {
        let first = tempfile::NamedTempFile::new().unwrap();
        let second = tempfile::NamedTempFile::new().unwrap();
        let batches = Arc::new(std::sync::Mutex::new(Vec::new()));
        let config = BenchmarkConfig {
            warmup_iterations: 0,
            benchmark_iterations: 3,
            benchmark_mode: BenchmarkMode::Batch,
            ..Default::default()
        };
        let adapter: Arc<dyn FrameworkAdapter> = Arc::new(RecordingBatchAdapter {
            batches: Arc::clone(&batches),
        });

        let results = BenchmarkRunner::run_batch_iterations_static(
            vec![first.path().to_path_buf(), second.path().to_path_buf()],
            adapter,
            &config,
            None,
            vec![false, false],
            vec![None, None],
            OutputFormat::Markdown,
        )
        .await
        .unwrap();

        assert_eq!(batches.lock().unwrap().len(), 3);
        assert_eq!(results.len(), 2);
        for result in results {
            assert_eq!(
                result
                    .iterations
                    .iter()
                    .map(|iteration| iteration.iteration)
                    .collect::<Vec<_>>(),
                vec![1, 2, 3]
            );
            assert_eq!(result.statistics.unwrap().sample_count, 3);
        }
    }

    #[tokio::test]
    async fn standard_single_and_batch_runners_populate_numeric_tf1_and_sf1() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("document.pdf"), b"pdf").unwrap();
        std::fs::write(temp.path().join("ground_truth.txt"), "ok").unwrap();
        std::fs::write(temp.path().join("ground_truth.md"), "ok").unwrap();
        let fixture_path = temp.path().join("fixture.json");
        std::fs::write(
            &fixture_path,
            serde_json::json!({
                "document": "document.pdf",
                "file_type": "pdf",
                "file_size": 3,
                "ground_truth": {
                    "text_file": "ground_truth.txt",
                    "markdown_file": "ground_truth.md",
                    "source": "manual"
                }
            })
            .to_string(),
        )
        .unwrap();

        for benchmark_mode in [BenchmarkMode::SingleFile, BenchmarkMode::Batch] {
            let mut registry = AdapterRegistry::new();
            registry
                .register(Arc::new(RecordingBatchAdapter {
                    batches: Arc::new(std::sync::Mutex::new(Vec::new())),
                }))
                .unwrap();
            let config = BenchmarkConfig {
                benchmark_mode,
                measure_quality: true,
                warmup_iterations: 0,
                benchmark_iterations: 1,
                ..Default::default()
            };
            let mut runner = BenchmarkRunner::new(config, registry);
            runner.load_fixtures(&fixture_path).unwrap();

            let results = runner.run(&["recording".to_string()]).await.unwrap();
            let quality = results[0]
                .quality
                .as_ref()
                .expect("quality populated by standard runner");
            assert_eq!(quality.f1_score_text, 1.0);
            assert_eq!(quality.f1_score_numeric, 1.0);
            assert_eq!(quality.f1_score_layout, Some(1.0));
        }
    }

    #[tokio::test]
    async fn test_run_with_no_frameworks() {
        let config = BenchmarkConfig::default();
        let registry = AdapterRegistry::new();
        let mut runner = BenchmarkRunner::new(config, registry);

        let result = runner.run(&[]).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No frameworks available"));
    }

    #[tokio::test]
    async fn test_run_fails_for_requested_unregistered_framework() {
        let config = BenchmarkConfig::default();
        let registry = AdapterRegistry::new();
        let mut runner = BenchmarkRunner::new(config, registry);

        let error = runner.run(&["missing".to_string()]).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains("requested framework 'missing' is not registered")
        );
    }

    #[tokio::test]
    async fn batch_mode_rejects_non_native_adapter_before_setup() {
        let setup_calls = Arc::new(AtomicUsize::new(0));
        let teardown_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = AdapterRegistry::new();
        registry
            .register(Arc::new(TeardownAdapter {
                name: "single-only",
                setup_calls: Arc::clone(&setup_calls),
                teardown_calls: Arc::clone(&teardown_calls),
                fail_setup: false,
                fail_teardown: false,
            }))
            .unwrap();
        let config = BenchmarkConfig {
            benchmark_mode: BenchmarkMode::Batch,
            ..Default::default()
        };
        let mut runner = BenchmarkRunner::new(config, registry);

        let error = runner.run(&["single-only".to_string()]).await.unwrap_err();

        assert!(
            error.to_string().contains("verified native batch API"),
            "unexpected error: {error}"
        );
        assert_eq!(setup_calls.load(Ordering::SeqCst), 0);
        assert_eq!(teardown_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn warmup_failure_aborts_run_without_recording_cold_start() {
        use crate::fixture::Fixture;

        let temp_dir = tempfile::tempdir().unwrap();
        let document_path = temp_dir.path().join("document.pdf");
        let fixture_path = temp_dir.path().join("fixture.json");
        std::fs::write(&document_path, b"pdf").unwrap();
        let fixture = Fixture {
            document: PathBuf::from("document.pdf"),
            file_type: "pdf".to_string(),
            file_size: 3,
            expected_frameworks: vec!["failed-warmup".to_string()],
            metadata: HashMap::new(),
            ground_truth: None,
        };
        std::fs::write(&fixture_path, serde_json::to_string(&fixture).unwrap()).unwrap();

        let teardown_calls = Arc::new(AtomicUsize::new(0));
        let mut registry = AdapterRegistry::new();
        registry
            .register(Arc::new(FailedWarmupAdapter {
                teardown_calls: Arc::clone(&teardown_calls),
            }))
            .unwrap();
        let config = BenchmarkConfig {
            benchmark_mode: BenchmarkMode::SingleFile,
            ..Default::default()
        };
        let mut runner = BenchmarkRunner::new(config, registry);
        runner.load_fixtures(&fixture_path).unwrap();

        let error = runner.run(&["failed-warmup".to_string()]).await.unwrap_err();

        assert!(error.to_string().contains("warmup failed for 'failed-warmup'"));
        assert!(error.to_string().contains("intentional warmup failure"));
        assert!(!runner.cold_start_durations.contains_key("failed-warmup"));
        assert_eq!(teardown_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn repeated_single_result_fails_if_any_iteration_fails_and_uses_success_payload() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let config = BenchmarkConfig {
            warmup_iterations: 0,
            benchmark_iterations: 2,
            ..Default::default()
        };
        let adapter: Arc<dyn FrameworkAdapter> = Arc::new(SequenceAdapter {
            calls: AtomicUsize::new(0),
        });

        let result = BenchmarkRunner::run_iterations_static(
            file.path(),
            adapter,
            &config,
            None,
            false,
            None,
            OutputFormat::Markdown,
        )
        .await
        .unwrap();

        assert!(!result.success);
        assert_eq!(result.error_kind, ErrorKind::FrameworkError);
        assert_eq!(result.extracted_text.as_deref(), Some("successful payload"));
    }

    #[tokio::test]
    async fn single_file_task_error_fails_run_instead_of_dropping_result() {
        let temp_dir = tempfile::tempdir().unwrap();
        let fixture_path = temp_dir.path().join("fixture.json");
        std::fs::write(temp_dir.path().join("document.pdf"), b"pdf").unwrap();
        let fixture = Fixture {
            document: PathBuf::from("document.pdf"),
            file_type: "pdf".to_string(),
            file_size: 3,
            expected_frameworks: vec!["task-error".to_string()],
            metadata: HashMap::new(),
            ground_truth: None,
        };
        std::fs::write(&fixture_path, serde_json::to_string(&fixture).unwrap()).unwrap();

        let mut registry = AdapterRegistry::new();
        registry
            .register(Arc::new(TaskErrorAdapter {
                calls: AtomicUsize::new(0),
            }))
            .unwrap();
        let config = BenchmarkConfig {
            benchmark_mode: BenchmarkMode::SingleFile,
            warmup_iterations: 0,
            benchmark_iterations: 1,
            ..Default::default()
        };
        let mut runner = BenchmarkRunner::new(config, registry);
        runner.load_fixtures(&fixture_path).unwrap();

        let error = runner.run(&["task-error".to_string()]).await.unwrap_err();

        assert!(error.to_string().contains("task-error"));
        assert!(error.to_string().contains("intentional task error"));
    }

    #[tokio::test]
    async fn repeated_batch_result_fails_if_any_iteration_fails_and_uses_success_payload() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let config = BenchmarkConfig {
            warmup_iterations: 0,
            benchmark_iterations: 2,
            ..Default::default()
        };
        let adapter: Arc<dyn FrameworkAdapter> = Arc::new(SequenceAdapter {
            calls: AtomicUsize::new(0),
        });

        let error = BenchmarkRunner::run_batch_iterations_static(
            vec![file.path().to_path_buf()],
            adapter,
            &config,
            None,
            vec![false],
            vec![None],
            OutputFormat::Markdown,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("partial batch failure"));
    }

    #[test]
    fn quality_ground_truth_fails_if_file_disappears_after_fixture_load() {
        use crate::fixture::{Fixture, GroundTruth};

        let temp_dir = tempfile::TempDir::new().unwrap();
        let fixture_path = temp_dir.path().join("fixture.json");
        let ground_truth_path = temp_dir.path().join("ground_truth.txt");
        std::fs::write(&ground_truth_path, "expected").unwrap();
        let fixture = Fixture {
            document: PathBuf::from("document.pdf"),
            file_type: "pdf".to_string(),
            file_size: 1,
            expected_frameworks: vec![],
            metadata: HashMap::new(),
            ground_truth: Some(GroundTruth {
                text_file: Some(PathBuf::from("ground_truth.txt")),
                markdown_file: None,
                fields_json: None,
                formulas_json: None,
                source: "manual".to_string(),
            }),
        };
        std::fs::write(&fixture_path, serde_json::to_string(&fixture).unwrap()).unwrap();
        let mut fixtures = FixtureManager::new();
        fixtures.load_fixture(&fixture_path).unwrap();
        std::fs::remove_file(ground_truth_path).unwrap();

        let error = load_quality_ground_truth(&fixtures).unwrap_err();
        assert!(error.to_string().contains("failed to read requested text ground truth"));
    }

    #[test]
    fn quality_ground_truth_uses_markdown_when_text_is_not_supplied() {
        use crate::fixture::{Fixture, GroundTruth};

        let temp_dir = tempfile::TempDir::new().unwrap();
        let fixture_path = temp_dir.path().join("fixture.json");
        std::fs::write(temp_dir.path().join("ground_truth.md"), "# Expected").unwrap();
        let fixture = Fixture {
            document: PathBuf::from("document.pdf"),
            file_type: "pdf".to_string(),
            file_size: 1,
            expected_frameworks: vec![],
            metadata: HashMap::new(),
            ground_truth: Some(GroundTruth {
                text_file: None,
                markdown_file: Some(PathBuf::from("ground_truth.md")),
                fields_json: None,
                formulas_json: None,
                source: "markdown_file".to_string(),
            }),
        };
        std::fs::write(&fixture_path, serde_json::to_string(&fixture).unwrap()).unwrap();
        let mut fixtures = FixtureManager::new();
        fixtures.load_fixture(&fixture_path).unwrap();

        let (text, markdown) = load_quality_ground_truth(&fixtures).unwrap();
        let document_path = temp_dir.path().join("document.pdf");
        assert_eq!(text.get(&document_path).map(String::as_str), Some("# Expected"));
        assert_eq!(markdown.get(&document_path).map(String::as_str), Some("# Expected"));
    }

    #[test]
    fn test_calculate_amplified_iterations() {
        assert_eq!(calculate_amplified_iterations(100, 1000), 10);
        assert_eq!(calculate_amplified_iterations(500, 1000), 2);
        assert_eq!(calculate_amplified_iterations(2000, 1000), 1);
        assert_eq!(calculate_amplified_iterations(0, 1000), 1);
        assert_eq!(calculate_amplified_iterations(1, 1000), 1000);
    }

    #[test]
    fn test_profiling_config_optimal_frequency() {
        assert_eq!(crate::ProfilingConfig::calculate_optimal_frequency(50), 500);
        assert_eq!(crate::ProfilingConfig::calculate_optimal_frequency(99), 500);

        assert_eq!(crate::ProfilingConfig::calculate_optimal_frequency(500), 500);

        assert_eq!(crate::ProfilingConfig::calculate_optimal_frequency(1000), 500);

        assert_eq!(crate::ProfilingConfig::calculate_optimal_frequency(5000), 100);

        assert_eq!(crate::ProfilingConfig::calculate_optimal_frequency(10000), 100);
    }

    #[test]
    fn test_profiling_config_validation() {
        let mut config = crate::ProfilingConfig::default();

        assert!(config.validate().is_ok());

        config.sampling_frequency = 50;
        assert!(config.validate().is_err());

        config.sampling_frequency = 20000;
        assert!(config.validate().is_err());

        config.sampling_frequency = 1000;
        assert!(config.validate().is_ok());

        config.batch_size = 0;
        assert!(config.validate().is_err());

        config.batch_size = 10;
        assert!(config.validate().is_ok());

        config.sample_count_threshold = 0;
        assert!(config.validate().is_err());

        config.sample_count_threshold = 500;
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_framework_size_enrichment_with_suffix_stripping() {
        use crate::types::{BenchmarkResult, FrameworkCapabilities, OcrStatus, PerformanceMetrics};
        use std::time::Duration;

        let config = BenchmarkConfig::default();
        let registry = AdapterRegistry::new();
        let runner = BenchmarkRunner::new(config, registry);

        let mut result_sync = BenchmarkResult {
            framework: "xberg-python-sync".to_string(),
            output_format: OutputFormat::Markdown,
            file_path: PathBuf::from("/test/file.pdf"),
            file_size: 1024,
            success: true,
            error_message: None,
            error_kind: ErrorKind::None,
            duration: Duration::from_millis(100),
            extraction_duration: None,
            subprocess_overhead: None,
            metrics: PerformanceMetrics::default(),
            quality: None,
            iterations: vec![],
            statistics: None,
            cold_start_duration: None,
            file_extension: "pdf".to_string(),
            framework_capabilities: FrameworkCapabilities::default(),
            pdf_metadata: None,
            ocr_status: OcrStatus::Unknown,
            extracted_text: None,
            system_load: None,
        };

        let mut result_async = BenchmarkResult {
            framework: "xberg-python-async".to_string(),
            output_format: OutputFormat::Markdown,
            file_path: PathBuf::from("/test/file.pdf"),
            file_size: 1024,
            success: true,
            error_message: None,
            error_kind: ErrorKind::None,
            duration: Duration::from_millis(100),
            extraction_duration: None,
            subprocess_overhead: None,
            metrics: PerformanceMetrics::default(),
            quality: None,
            iterations: vec![],
            statistics: None,
            cold_start_duration: None,
            file_extension: "pdf".to_string(),
            framework_capabilities: FrameworkCapabilities::default(),
            pdf_metadata: None,
            ocr_status: OcrStatus::Unknown,
            extracted_text: None,
            system_load: None,
        };

        let mut result_batch = BenchmarkResult {
            framework: "xberg-python-batch".to_string(),
            output_format: OutputFormat::Markdown,
            file_path: PathBuf::from("/test/file.pdf"),
            file_size: 1024,
            success: true,
            error_message: None,
            error_kind: ErrorKind::None,
            duration: Duration::from_millis(100),
            extraction_duration: None,
            subprocess_overhead: None,
            metrics: PerformanceMetrics::default(),
            quality: None,
            iterations: vec![],
            statistics: None,
            cold_start_duration: None,
            file_extension: "pdf".to_string(),
            framework_capabilities: FrameworkCapabilities::default(),
            pdf_metadata: None,
            ocr_status: OcrStatus::Unknown,
            extracted_text: None,
            system_load: None,
        };

        assert!(result_sync.framework_capabilities.installation_size.is_none());
        assert!(result_async.framework_capabilities.installation_size.is_none());
        assert!(result_batch.framework_capabilities.installation_size.is_none());

        runner.enrich_with_framework_size(&mut result_sync);
        runner.enrich_with_framework_size(&mut result_async);
        runner.enrich_with_framework_size(&mut result_batch);

        if let Some(size_info) = &result_sync.framework_capabilities.installation_size {
            assert!(size_info.size_bytes > 0, "Size should be positive");
            assert!(!size_info.method.is_empty(), "Method should be set");
            assert!(!size_info.description.is_empty(), "Description should be set");

            assert!(result_async.framework_capabilities.installation_size.is_some());
            assert!(result_batch.framework_capabilities.installation_size.is_some());

            let async_size = result_async.framework_capabilities.installation_size.as_ref().unwrap();
            let batch_size = result_batch.framework_capabilities.installation_size.as_ref().unwrap();

            assert_eq!(async_size.size_bytes, size_info.size_bytes);
            assert_eq!(batch_size.size_bytes, size_info.size_bytes);
        }
    }
}
