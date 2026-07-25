//! Benchmark harness CLI

#[cfg(feature = "memory-profiling")]
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use benchmark_harness::{BenchmarkConfig, BenchmarkMode, FixtureManager, OutputFormat, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr as _;

/// CLI enum for benchmark mode
#[derive(Debug, Clone, Copy, ValueEnum)]
enum CliMode {
    /// Single-file mode: Sequential execution for fair latency comparison
    SingleFile,
    /// Batch mode: Verified native framework batch APIs for throughput measurement
    Batch,
}

/// CLI enum for output file format
#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFileFormat {
    /// JSON format (default)
    Json,
}

impl From<CliMode> for BenchmarkMode {
    fn from(mode: CliMode) -> Self {
        match mode {
            CliMode::SingleFile => BenchmarkMode::SingleFile,
            CliMode::Batch => BenchmarkMode::Batch,
        }
    }
}

fn normalize_run_frameworks(frameworks: &[String], batch_mode: bool) -> Vec<String> {
    let mut normalized = Vec::with_capacity(frameworks.len());
    for framework in frameworks {
        let name = if batch_mode && framework.starts_with("xberg-") && !framework.ends_with("-batch") {
            format!("{framework}-batch")
        } else {
            framework.clone()
        };
        if !normalized.contains(&name) {
            normalized.push(name);
        }
    }
    normalized
}

fn parse_model_provenance(values: &[String]) -> Result<Vec<benchmark_harness::ModelProvenance>> {
    values
        .iter()
        .map(|value| {
            let (framework, identifier) = value.split_once('=').ok_or_else(|| {
                benchmark_harness::Error::Config(format!(
                    "invalid model identity '{value}': expected FRAMEWORK=OWNER/REPOSITORY@REVISION#DIGEST"
                ))
            })?;
            let valid_component = |value: &str| {
                !value.is_empty()
                    && value
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
            };
            let structured = identifier
                .split_once('#')
                .and_then(|(repository_revision, digest)| {
                    repository_revision
                        .split_once('@')
                        .map(|(repository, revision)| (repository, revision, digest))
                })
                .is_some_and(|(repository, revision, digest)| {
                    let mut repository_parts = repository.split('/');
                    valid_component(repository_parts.next().unwrap_or_default())
                        && valid_component(repository_parts.next().unwrap_or_default())
                        && repository_parts.next().is_none()
                        && valid_component(revision)
                        && !digest.is_empty()
                        && digest.chars().all(|character| {
                            character.is_ascii_alphanumeric() || matches!(character, ':' | '-' | '_' | '.')
                        })
                });
            if framework.is_empty() || !structured {
                return Err(benchmark_harness::Error::Config(format!(
                    "invalid model identity '{value}': expected path-free FRAMEWORK=OWNER/REPOSITORY@REVISION#DIGEST"
                )));
            }
            Ok(benchmark_harness::ModelProvenance {
                framework: framework.to_string(),
                identifier: identifier.to_string(),
            })
        })
        .collect()
}

#[derive(Parser)]
#[command(name = "benchmark-harness")]
#[command(about = "Benchmark harness for document extraction frameworks", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List all fixtures from a directory
    ListFixtures {
        /// Directory or file pattern to search for fixtures
        #[arg(short, long)]
        fixtures: PathBuf,
    },

    /// Validate fixtures without running benchmarks
    Validate {
        /// Directory or file pattern to search for fixtures
        #[arg(short, long)]
        fixtures: PathBuf,
    },

    /// Run benchmarks
    Run {
        /// Fixture directory, or a single fixture when --cohort is omitted
        #[arg(short, long)]
        fixtures: PathBuf,

        /// Exact ordered cohort manifest. Absolute paths are used directly; existing
        /// relative paths use the current directory; others use the fixture directory.
        #[arg(long)]
        cohort: Option<PathBuf>,

        /// Require complete native batches of exactly this many documents
        #[arg(long)]
        batch_size: Option<usize>,

        /// Frameworks to benchmark (comma-separated)
        #[arg(short = 'F', long, value_delimiter = ',')]
        frameworks: Vec<String>,

        /// Output directory for results
        #[arg(short, long, default_value = "results")]
        output: PathBuf,

        /// Maximum concurrent extractions
        #[arg(short = 'c', long)]
        max_concurrent: Option<usize>,

        /// Xberg's configured extraction thread budget.
        ///
        /// Single-file mode otherwise uses Xberg's automatic budget; batch mode
        /// otherwise defaults to --max-concurrent. Does not affect other frameworks.
        #[arg(long)]
        xberg_max_threads: Option<usize>,

        /// Timeout in seconds
        #[arg(short = 't', long)]
        timeout: Option<u64>,

        /// Benchmark mode: single-file (sequential) or batch (concurrent)
        #[arg(short = 'm', long, value_enum, default_value = "batch")]
        mode: CliMode,

        /// Number of warmup iterations (discarded from statistics)
        #[arg(short = 'w', long, default_value = "1")]
        warmup: usize,

        /// Number of benchmark iterations for statistical analysis
        #[arg(short = 'i', long, default_value = "3")]
        iterations: usize,

        /// Enable OCR for image extraction
        #[arg(long, default_value = "false")]
        ocr: bool,

        /// Enable quality assessment
        #[arg(long, default_value = "false")]
        measure_quality: bool,

        /// Output format for extraction: markdown, plaintext, or both (default: markdown)
        #[arg(long, default_value = "markdown")]
        output_format: String,

        /// Run only a subset of fixtures (format: INDEX/TOTAL, e.g. 1/3 for first of 3 shards)
        #[arg(long)]
        shard: Option<String>,

        /// Model identity as FRAMEWORK=OWNER/REPOSITORY@REVISION#DIGEST (repeatable)
        #[arg(long = "model-id")]
        model_ids: Vec<String>,
    },

    /// Consolidate multiple benchmark runs
    Consolidate {
        /// Input directories containing benchmark results
        #[arg(short, long, value_delimiter = ',')]
        inputs: Vec<PathBuf>,

        /// Output directory for consolidated results
        #[arg(short, long)]
        output: PathBuf,

        /// Baseline framework for delta calculations (not used but provided for compatibility)
        #[arg(long, default_value = "xberg-rust")]
        baseline: String,
    },

    /// Measure framework installation sizes
    MeasureFrameworkSizes {
        /// Output JSON file for framework sizes
        #[arg(long)]
        output: PathBuf,
    },

    /// Build a per-document gap report from a `run` result set.
    ///
    /// Pivots `results.json` by document and ranks the documents where
    /// competitors beat our heuristics path, split by text (TF1) vs structure
    /// (SF1). Writes `per_document.json` + `gaps.md`.
    GapReport {
        /// Directory containing `results.json` (as produced by `run`)
        #[arg(short, long, default_value = "results")]
        results: PathBuf,

        /// Output directory for `per_document.json` + `gaps.md` (defaults to the results dir)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Our model-free heuristics-path framework row
        #[arg(long, default_value = "xberg-markdown-baseline")]
        baseline: String,

        /// Our routed ML-layout framework row
        #[arg(long, default_value = "xberg-markdown-layout")]
        layout: String,

        /// Competitor frameworks to rank against (comma-separated)
        #[arg(long, value_delimiter = ',', default_value = "liteparse,docling")]
        competitors: Vec<String>,
    },

    /// Compare extraction pipelines on document corpus with quality scoring
    Compare {
        /// Directory containing fixture JSON files
        #[arg(short, long)]
        fixtures: PathBuf,

        /// Pipelines to compare (comma-separated: baseline,layout,tesseract,paddle,docling)
        #[arg(long, value_delimiter = ',')]
        pipelines: Option<Vec<String>>,

        /// Dump extraction outputs to /tmp/xberg_compare/
        #[arg(long)]
        dump_outputs: bool,

        /// Enable quality guardrails (fail on regressions)
        #[arg(long)]
        guardrails: bool,

        /// Path to guardrails JSON config file (used when --guardrails is set)
        #[arg(long, default_value = "guardrails.json")]
        guardrails_file: PathBuf,

        /// Only run documents whose name contains this string
        #[arg(long)]
        filter: Option<String>,

        /// Write full comparison results to JSON file
        #[arg(long)]
        json_output: Option<PathBuf>,

        /// Run noise detection on extracted outputs
        #[arg(long)]
        noise: bool,

        /// Enable diagnostic diff mode for poor-scoring documents
        #[arg(long)]
        diagnose: bool,

        /// SF1 threshold below which to diagnose (default 0.8)
        #[arg(long, default_value = "0.8")]
        diagnose_threshold: f64,
    },

    /// Generate quality guardrails JSON from benchmark results
    GenerateGuardrails {
        /// Directory containing fixture JSON files
        #[arg(short, long)]
        fixtures: PathBuf,

        /// Pipelines to run (comma-separated)
        #[arg(long, value_delimiter = ',')]
        pipelines: Option<Vec<String>>,

        /// Threshold factor applied to observed scores (e.g. 0.9 = 90% of observed)
        #[arg(long, default_value = "0.9")]
        threshold_factor: f64,

        /// Output path for the guardrails JSON file
        #[arg(short, long, default_value = "guardrails.json")]
        output: PathBuf,
    },

    /// Run 6-path pipeline benchmark across the document corpus
    PipelineBenchmark {
        /// Directory containing fixture JSON files
        #[arg(short, long)]
        fixtures: PathBuf,

        /// Pipeline paths to run (comma-separated: baseline,layout,tesseract,tesseract+layout,paddle,paddle+layout)
        #[arg(long, value_delimiter = ',')]
        paths: Option<Vec<String>>,

        /// Also run documents whose name contains one of these strings; unions with --group
        #[arg(long, value_delimiter = ',')]
        doc: Option<Vec<String>>,

        /// Run a named benchmark group (hotspot, smoke, promotion, holdout, tables, structure, lists)
        #[arg(long)]
        group: Option<String>,

        /// Dump outputs to /tmp/xberg_pipeline/
        #[arg(long)]
        dump_outputs: bool,

        /// Write JSON results to this file
        #[arg(long)]
        json_output: Option<PathBuf>,

        /// Sort results by metric for triage (sf1, tf1, time)
        #[arg(long, default_value = "sf1")]
        sort_by: String,

        /// Show only the bottom N worst-performing documents
        #[arg(long)]
        bottom_n: Option<usize>,

        /// Print per-block-type F1 breakdown for triage
        #[arg(long)]
        triage_blocks: bool,

        /// Generate per-pipeline flamegraph SVGs in this directory
        #[arg(long)]
        profile_dir: Option<PathBuf>,
    },

    /// Corpus-wide extraction survey with stats
    Survey {
        /// Directory containing fixture JSON files
        #[arg(short, long)]
        fixtures: PathBuf,

        /// File types to include (comma-separated, e.g. pdf,docx)
        #[arg(long, value_delimiter = ',')]
        types: Option<Vec<String>>,
    },

    /// Layout model A/B comparison benchmark
    ModelBenchmark {
        /// Directory containing fixture JSON files
        #[arg(short, long)]
        fixtures: PathBuf,

        /// First table model name (e.g. "tatr", "slanet_wired", "slanet_auto")
        #[arg(long, default_value = "tatr")]
        model_a: String,

        /// Second table model name (e.g. "tatr", "slanet_wired", "slanet_auto")
        #[arg(long, default_value = "slanet_auto")]
        model_b: String,
    },

    /// Multi-document PDF split-boundary benchmark (Auto boundary accuracy,
    /// reconstruction fidelity, single-parse timing)
    SplitBenchmark {
        /// Directory scanned recursively for `*.split.json` manifests
        #[arg(short, long, default_value = "tools/benchmark-harness/fixtures/split")]
        fixtures: PathBuf,

        /// Also run the MultidocThresholds sweep grid
        #[arg(long)]
        sweep: bool,

        /// Write a split-boundary-guardrails.json to this path (from default-threshold results)
        #[arg(long)]
        guardrails_out: Option<PathBuf>,

        /// Generate a CPU flamegraph SVG for the single-parse path at this path
        #[arg(long)]
        profile_out: Option<PathBuf>,
    },

    /// Embedding throughput and batch-size benchmark across all presets
    EmbedBenchmark,

    /// Validate ground truth files and optionally fix HTML artifacts
    ValidateGt {
        /// Directory containing fixture JSON files
        #[arg(short, long)]
        fixtures: PathBuf,

        /// Auto-fix HTML tags in markdown ground truth files
        #[arg(long)]
        fix: bool,

        /// Fail (non-zero exit) if any fixture cannot load its ground truth — e.g. the
        /// reference-corpus cache was not restored. Used as a fast CI pre-check.
        #[arg(long)]
        strict: bool,
    },

    /// Compute field-level extraction quality (form-fields, formula, structured)
    FieldQuality {
        /// Directory containing fixture JSON files
        #[arg(short, long)]
        fixtures: PathBuf,

        /// Extraction mode: form-fields, formula, or structured
        #[arg(long, default_value = "form-fields")]
        mode: String,

        /// Dataset name for structured mode (cord, sroie, funsd, docile, vrdu)
        #[arg(long)]
        dataset: Option<String>,

        /// Only run fixtures whose file stem contains this string
        #[arg(long)]
        filter: Option<String>,
    },

    /// Score precomputed Markdown outputs without running extraction
    ScoreOutputs {
        /// Directory containing fixture JSON files, or one fixture JSON file
        #[arg(short, long)]
        fixtures: PathBuf,

        /// Markdown output directory, Markdown file, JSON map/records, or JSONL records
        #[arg(short, long)]
        outputs: PathBuf,

        /// Write the score report to this JSON file instead of standard output
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let rust_log = std::env::var("RUST_LOG").ok();
    let benchmark_debug = std::env::var_os("BENCHMARK_DEBUG").is_some();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_filter(rust_log.as_deref(), benchmark_debug))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::ListFixtures { fixtures } => {
            let mut manager = FixtureManager::new();

            if fixtures.is_dir() {
                manager.load_fixtures_from_dir(&fixtures)?;
            } else {
                manager.load_fixture(&fixtures)?;
            }

            println!("Loaded {} fixture(s)", manager.len());
            for (path, fixture) in manager.fixtures() {
                println!(
                    "  {} - {} ({} bytes)",
                    path.display(),
                    fixture.document.display(),
                    fixture.file_size
                );
            }

            Ok(())
        }

        Commands::Validate { fixtures } => {
            let mut manager = FixtureManager::new();

            if fixtures.is_dir() {
                manager.load_fixtures_from_dir(&fixtures)?;
            } else {
                manager.load_fixture(&fixtures)?;
            }

            println!("✓ All {} fixture(s) are valid", manager.len());
            Ok(())
        }

        Commands::Run {
            fixtures,
            cohort,
            batch_size,
            frameworks,
            output,
            max_concurrent,
            xberg_max_threads,
            timeout,
            mode,
            warmup,
            iterations,
            ocr,
            measure_quality,
            output_format,
            shard,
            model_ids,
        } => {
            use benchmark_harness::{AdapterRegistry, BenchmarkRunner};
            use std::sync::Arc;

            for framework in &frameworks {
                if !framework.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
                    return Err(benchmark_harness::Error::Benchmark(format!(
                        "Invalid framework name '{}': must contain only alphanumeric characters, hyphens, or underscores",
                        framework
                    )));
                }
            }

            let config = BenchmarkConfig {
                output_dir: output.clone(),
                max_concurrent: max_concurrent.unwrap_or_else(num_cpus::get),
                xberg_max_threads,
                timeout: std::time::Duration::from_secs(timeout.unwrap_or(1800)),
                benchmark_mode: mode.into(),
                warmup_iterations: warmup,
                benchmark_iterations: iterations,
                measure_quality,
                ocr_enabled: ocr,
                ..Default::default()
            };

            config.validate()?;

            let parsed_format = OutputFormat::from_str(&output_format).map_err(benchmark_harness::Error::Config)?;
            let batch_mode = matches!(config.benchmark_mode, BenchmarkMode::Batch);
            let frameworks = normalize_run_frameworks(&frameworks, batch_mode);
            let model_provenance = parse_model_provenance(&model_ids)?;

            let mut registry = AdapterRegistry::new();

            let should_init = |name: &str| -> bool { frameworks.is_empty() || frameworks.iter().any(|f| f == name) };

            macro_rules! try_register {
                ($name:expr, $create_fn:expr, $count:expr) => {
                    if should_init($name) {
                        match $create_fn() {
                            Ok(adapter) => {
                                if let Err(err) = registry.register(Arc::new(adapter)) {
                                    tracing::warn!(
                                        framework = $name,
                                        error = %err,
                                        "adapter registration failed"
                                    );
                                } else {
                                    tracing::info!(framework = $name, "adapter registered");
                                    $count += 1;
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    framework = $name,
                                    error = %err,
                                    "adapter initialization failed"
                                );
                            }
                        }
                    }
                };
            }

            use benchmark_harness::XbergPipeline;
            use benchmark_harness::adapters::create_xberg_adapter;

            let mut xberg_count = 0;
            let pipelines = [XbergPipeline::Baseline, XbergPipeline::Layout, XbergPipeline::PaddleOcr];
            let formats = [parsed_format];
            for pipeline in &pipelines {
                if !ocr && matches!(pipeline, XbergPipeline::PaddleOcr) {
                    continue;
                }
                for format in &formats {
                    let format_slug = match format {
                        OutputFormat::Markdown => "markdown",
                        OutputFormat::Plaintext => "plaintext",
                    };
                    let base_name = format!("xberg-{}-{}", format_slug, pipeline.as_str());
                    let framework_name = if batch_mode {
                        format!("{base_name}-batch")
                    } else {
                        base_name
                    };
                    if should_init(&framework_name) {
                        match create_xberg_adapter(*pipeline, *format, batch_mode, ocr)
                            .map(|adapter| adapter.with_batch_workers(config.max_concurrent))
                            .map(|adapter| match config.xberg_max_threads {
                                Some(max_threads) => adapter.with_xberg_max_threads(max_threads),
                                None => adapter,
                            }) {
                            Ok(adapter) => {
                                if let Err(err) = registry.register(Arc::new(adapter)) {
                                    tracing::warn!(
                                        framework = %framework_name,
                                        error = %err,
                                        "adapter registration failed"
                                    );
                                } else {
                                    tracing::info!(framework = %framework_name, "adapter registered");
                                    xberg_count += 1;
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    framework = %framework_name,
                                    error = %err,
                                    "adapter initialization failed"
                                );
                            }
                        }
                    }
                }
            }

            let total_requested = if frameworks.is_empty() {
                if ocr { 3 } else { 2 }
            } else {
                frameworks.iter().filter(|f| f.contains("xberg")).count()
            };
            tracing::info!(
                available = xberg_count,
                requested = total_requested,
                "Xberg adapters available"
            );

            let mut external_count = 0;

            if !matches!(config.benchmark_mode, BenchmarkMode::Batch) {
                use benchmark_harness::adapters::{
                    create_docling_adapter, create_liteparse_adapter, create_markitdown_adapter, create_mineru_adapter,
                    create_pymupdf4llm_adapter, create_tika_adapter, create_unstructured_adapter,
                };

                try_register!("docling", || create_docling_adapter(ocr), external_count);
                try_register!("markitdown", || create_markitdown_adapter(ocr), external_count);
                try_register!("unstructured", || create_unstructured_adapter(ocr), external_count);
                try_register!("tika", || create_tika_adapter(ocr), external_count);
                try_register!("pymupdf4llm", || create_pymupdf4llm_adapter(ocr), external_count);
                try_register!("mineru", || create_mineru_adapter(ocr), external_count);
                try_register!(
                    "liteparse",
                    || create_liteparse_adapter(ocr).map(|adapter| adapter.with_batch_workers(config.max_concurrent)),
                    external_count
                );
            } else {
                use benchmark_harness::adapters::{create_docling_adapter, create_liteparse_adapter};
                try_register!("docling", || create_docling_adapter(ocr), external_count);
                try_register!(
                    "liteparse",
                    || create_liteparse_adapter(ocr).map(|adapter| adapter.with_batch_workers(config.max_concurrent)),
                    external_count
                );
                tracing::debug!(
                    docling_api = "convert_all",
                    docling_execution = "cold end-to-end subprocess",
                    liteparse_api = "batch-parse",
                    "verified batch APIs"
                );
                tracing::debug!(
                    reason = "native batch behavior is unverified",
                    "other external frameworks skipped"
                );
            }

            tracing::info!(
                available = external_count,
                supported = 7,
                "open source extraction adapters available"
            );
            tracing::info!(available = xberg_count + external_count, "total adapters available");

            // NOTE: This check must run AFTER all adapters (xberg + external) are registered
            let mut failed_frameworks = Vec::new();
            for name in &frameworks {
                if !registry.contains(name) {
                    failed_frameworks.push(name.clone());
                }
            }
            if !failed_frameworks.is_empty() {
                return Err(benchmark_harness::Error::Config(format!(
                    "{} requested framework(s) are unavailable: {}",
                    failed_frameworks.len(),
                    failed_frameworks.join(", ")
                )));
            }

            let mut runner = BenchmarkRunner::with_output_format(config, registry, parsed_format);
            let cohort_manifest = if let Some(manifest_path) = cohort.as_deref() {
                Some(runner.load_cohort(&fixtures, manifest_path)?)
            } else {
                runner.load_fixtures(&fixtures)?;
                None
            };

            if batch_size.is_some() && !batch_mode {
                return Err(benchmark_harness::Error::Config(
                    "fixed batch sizing requires --mode batch".to_string(),
                ));
            }
            let fixed_batch_size = if batch_mode {
                match (cohort_manifest.as_ref(), batch_size) {
                    (Some(manifest), Some(requested)) if requested != manifest.batch_size => {
                        return Err(benchmark_harness::Error::Config(format!(
                            "--batch-size {requested} does not match cohort batch_size {}",
                            manifest.batch_size
                        )));
                    }
                    (Some(manifest), _) => Some(manifest.batch_size),
                    (None, requested) => requested,
                }
            } else {
                None
            };
            if let Some(size) = fixed_batch_size {
                runner.set_fixed_batch_size(size)?;
            }

            if (cohort.is_some() || fixed_batch_size.is_some()) && shard.is_some() {
                return Err(benchmark_harness::Error::Config(
                    "--shard cannot be combined with exact cohort or fixed batch sizing".to_string(),
                ));
            }

            if let Some(ref shard_spec) = shard {
                let parts: Vec<&str> = shard_spec.split('/').collect();
                if parts.len() != 2 {
                    return Err(benchmark_harness::Error::Config(format!(
                        "Invalid shard format '{}': expected INDEX/TOTAL (e.g. 1/3)",
                        shard_spec
                    )));
                }
                let index: usize = parts[0].parse().map_err(|_| {
                    benchmark_harness::Error::Config(format!("Invalid shard index '{}': must be a number", parts[0]))
                })?;
                let total: usize = parts[1].parse().map_err(|_| {
                    benchmark_harness::Error::Config(format!("Invalid shard total '{}': must be a number", parts[1]))
                })?;
                if index < 1 || index > total || total < 1 {
                    return Err(benchmark_harness::Error::Config(format!(
                        "Invalid shard {}/{}: index must be 1..=total",
                        index, total
                    )));
                }
                let total_before = runner.fixture_count();
                runner.apply_shard(index, total);
                println!(
                    "Shard {}/{}: {} of {} fixtures",
                    index,
                    total,
                    runner.fixture_count(),
                    total_before
                );
            }

            println!("Loaded {} fixture(s)", runner.fixture_count());
            println!("Frameworks: {:?}", frameworks);
            println!("Configuration: {:?}", runner.config());

            if runner.fixture_count() == 0 {
                println!("No fixtures to benchmark");
                return Ok(());
            }

            let provenance = runner.capture_provenance(
                &frameworks,
                &fixtures,
                cohort_manifest.as_ref(),
                cohort.as_deref(),
                fixed_batch_size,
                &model_provenance,
            )?;

            println!("\nRunning benchmarks...");
            let results = runner.run(&frameworks).await?;

            println!("\nCompleted {} benchmark(s)", results.len());

            let mut success_count = 0;
            let mut failure_count = 0;

            for result in &results {
                if result.success {
                    success_count += 1;
                } else {
                    failure_count += 1;
                }
            }

            println!("\nSummary:");
            println!("  Successful: {}", success_count);
            println!("  Failed: {}", failure_count);
            println!("  Total: {}", results.len());

            use benchmark_harness::{write_by_extension_analysis, write_json};

            let output_file = output.join("results.json");
            write_json(&results, &output_file)?;
            println!("\nResults written to: {}", output_file.display());

            let by_ext_file = output.join("by-extension.json");
            write_by_extension_analysis(&results, &by_ext_file)?;
            println!("Per-extension analysis written to: {}", by_ext_file.display());

            let provenance_file = output.join("provenance.json");
            benchmark_harness::write_run_provenance(&provenance, &provenance_file)?;
            println!("Run provenance written to: {}", provenance_file.display());

            if !failed_frameworks.is_empty() {
                return Err(benchmark_harness::Error::Benchmark(format!(
                    "Requested framework(s) failed to initialize: {}",
                    failed_frameworks.join(", ")
                )));
            }

            if results.is_empty() {
                return Err(benchmark_harness::Error::Benchmark(
                    "No benchmark results were produced".to_string(),
                ));
            }

            if failure_count > 0 {
                return Err(benchmark_harness::Error::Benchmark(format!(
                    "{} of {} extraction(s) failed; partial benchmark runs are not valid headline results",
                    failure_count,
                    results.len(),
                )));
            }

            Ok(())
        }
        Commands::Consolidate {
            inputs,
            output,
            baseline: _baseline,
        } => {
            use benchmark_harness::load_run_results;

            if inputs.is_empty() {
                return Err(benchmark_harness::Error::Benchmark(
                    "No input directories specified".to_string(),
                ));
            }

            println!("Loading benchmark results from {} directory(ies)...", inputs.len());

            let mut all_results = Vec::new();
            for input in &inputs {
                if !input.is_dir() {
                    return Err(benchmark_harness::Error::Benchmark(format!(
                        "Input path is not a directory: {}",
                        input.display()
                    )));
                }
                println!("  Loading from: {}", input.display());
                let run_results = load_run_results(input)?;
                println!("    Loaded {} results", run_results.len());
                all_results.extend(run_results);
            }

            println!("\nAggregating {} results...", all_results.len());
            let aggregated = benchmark_harness::aggregate_new_format(&all_results);
            println!(
                "  Aggregated {} frameworks across {} file types",
                aggregated.by_framework_mode.len(),
                aggregated
                    .by_framework_mode
                    .values()
                    .flat_map(|fm| fm.by_file_type.keys())
                    .collect::<HashSet<_>>()
                    .len()
            );

            for agg in aggregated.by_framework_mode.values() {
                if let Some(cs) = &agg.cold_start {
                    tracing::info!(
                        framework = %agg.framework,
                        mode = %agg.mode,
                        file_type_count = agg.by_file_type.len(),
                        cold_start_p50_ms = cs.p50_ms,
                        "framework summary"
                    );
                } else {
                    tracing::info!(
                        framework = %agg.framework,
                        mode = %agg.mode,
                        file_type_count = agg.by_file_type.len(),
                        "framework summary"
                    );
                }
            }

            std::fs::create_dir_all(&output).map_err(benchmark_harness::Error::Io)?;

            let output_file = output.join("aggregated.json");
            let json = serde_json::to_string_pretty(&aggregated)
                .map_err(|e| benchmark_harness::Error::Benchmark(format!("Failed to serialize results: {}", e)))?;
            std::fs::write(&output_file, json).map_err(benchmark_harness::Error::Io)?;
            println!("\nResults written to: {}", output_file.display());

            Ok(())
        }

        Commands::Compare {
            fixtures,
            pipelines,
            dump_outputs,
            guardrails,
            guardrails_file,
            filter,
            json_output,
            noise,
            diagnose,
            diagnose_threshold,
        } => {
            use benchmark_harness::comparison::{ComparisonConfig, Pipeline, run_with_guardrails};

            let selected_pipelines = match pipelines {
                Some(names) => names.iter().filter_map(|n| Pipeline::parse(n)).collect(),
                None => vec![Pipeline::Baseline, Pipeline::Layout],
            };

            let config = ComparisonConfig {
                fixtures_dir: fixtures,
                pipelines: selected_pipelines,
                dump_outputs,
                guardrails,
                guardrails_file: Some(guardrails_file),
                name_filter: filter,
                json_output,
                noise,
                diagnose,
                diagnose_threshold,
            };

            let exit_code = run_with_guardrails(&config).await?;
            if exit_code != 0 {
                std::process::exit(exit_code);
            }
            Ok(())
        }

        Commands::GenerateGuardrails {
            fixtures,
            pipelines,
            threshold_factor,
            output,
        } => {
            use benchmark_harness::comparison::{ComparisonConfig, Pipeline, generate_guardrails, run_comparison};

            let selected_pipelines = match pipelines {
                Some(names) => names.iter().filter_map(|n| Pipeline::parse(n)).collect(),
                None => vec![Pipeline::Baseline, Pipeline::Layout],
            };

            let config = ComparisonConfig {
                fixtures_dir: fixtures,
                pipelines: selected_pipelines,
                dump_outputs: false,
                guardrails: false,
                guardrails_file: None,
                name_filter: None,
                json_output: None,
                noise: false,
                diagnose: false,
                diagnose_threshold: 0.8,
            };

            let results = run_comparison(&config).await?;
            let guardrails = generate_guardrails(&results, threshold_factor);
            let json = serde_json::to_string_pretty(&guardrails)
                .map_err(|e| benchmark_harness::Error::Benchmark(format!("Failed to serialize guardrails: {}", e)))?;
            std::fs::write(&output, json).map_err(benchmark_harness::Error::Io)?;
            tracing::info!(
                guardrail_count = guardrails.contracts.len(),
                document_count = results.len(),
                output = %output.display(),
                "guardrails generated"
            );
            Ok(())
        }

        Commands::PipelineBenchmark {
            fixtures,
            paths,
            doc,
            group,
            dump_outputs,
            json_output,
            sort_by,
            bottom_n,
            triage_blocks,
            profile_dir,
        } => {
            use benchmark_harness::comparison::Pipeline;
            use benchmark_harness::pipeline_benchmark::{
                PipelineBenchmarkConfig, SortMetric, default_paths, print_pipeline_table, print_triage_blocks,
                run_pipeline_benchmark, write_json_output_with_config,
            };

            let selected_paths = match paths {
                Some(names) => names.iter().filter_map(|n| Pipeline::parse(n)).collect(),
                None => default_paths(),
            };

            let sort_metric = SortMetric::parse(&sort_by).unwrap_or_default();

            let (doc_filter, exact_doc_filter) = {
                let patterns: Vec<String> = doc.unwrap_or_default();
                let mut exact_names = Vec::new();
                if let Some(ref group_name) = group {
                    use benchmark_harness::groups::{find_group, group_names, resolve_group_docs};
                    let g = find_group(group_name).ok_or_else(|| {
                        benchmark_harness::Error::Config(format!(
                            "Unknown group '{}'. Available: {}",
                            group_name,
                            group_names().join(", ")
                        ))
                    })?;
                    exact_names = resolve_group_docs(&fixtures, g)?;
                    tracing::info!(
                        group = g.name,
                        description = g.description,
                        matched_document_count = exact_names.len(),
                        "benchmark group resolved"
                    );
                }
                (patterns, exact_names)
            };

            if let Some(ref prof_dir) = profile_dir {
                use benchmark_harness::profiling::ProfileGuard;

                std::fs::create_dir_all(prof_dir).map_err(benchmark_harness::Error::Io)?;

                for &pipeline in &selected_paths {
                    let svg_path = prof_dir.join(format!("{}.svg", pipeline.name()));
                    tracing::info!(
                        pipeline = pipeline.name(),
                        output = %svg_path.display(),
                        "profiling pipeline"
                    );

                    let config = PipelineBenchmarkConfig {
                        fixtures_dir: fixtures.clone(),
                        paths: vec![pipeline],
                        doc_filter: doc_filter.clone(),
                        exact_doc_filter: exact_doc_filter.clone(),
                        dump_outputs,
                        json_output: None,
                        sort_by: sort_metric,
                        bottom_n: None,
                        triage_blocks: false,
                    };

                    let guard = ProfileGuard::new(1000)?;
                    let results = run_pipeline_benchmark(&config).await?;
                    let profiling_result = guard.finish()?;
                    profiling_result.generate_flamegraph(&svg_path)?;

                    print_pipeline_table(&results, sort_metric, None);
                }

                return Ok(());
            }

            let config = PipelineBenchmarkConfig {
                fixtures_dir: fixtures,
                paths: selected_paths,
                doc_filter: doc_filter.clone(),
                exact_doc_filter,
                dump_outputs,
                json_output: json_output.clone(),
                sort_by: sort_metric,
                bottom_n,
                triage_blocks,
            };

            let results = run_pipeline_benchmark(&config).await?;
            print_pipeline_table(&results, sort_metric, bottom_n);

            if triage_blocks {
                print_triage_blocks(&results, sort_metric, bottom_n.unwrap_or(10));
            }

            if let Some(ref path) = json_output {
                write_json_output_with_config(&results, path, &config)?;
            }

            Ok(())
        }

        Commands::Survey { fixtures, types } => {
            use benchmark_harness::survey::{SurveyConfig, print_survey_table, run_survey};

            let config = SurveyConfig {
                fixtures_dir: fixtures,
                file_types: types,
            };

            let results = run_survey(&config).await?;
            print_survey_table(&results);
            Ok(())
        }

        Commands::ModelBenchmark {
            fixtures,
            model_a,
            model_b,
        } => {
            use benchmark_harness::model_benchmark::{ModelBenchmarkConfig, print_model_table, run_model_benchmark};

            let config = ModelBenchmarkConfig {
                fixtures_dir: fixtures,
                model_a: model_a.clone(),
                model_b: model_b.clone(),
                ..Default::default()
            };

            let results = run_model_benchmark(&config).await?;
            print_model_table(&results, &model_a, &model_b);
            Ok(())
        }

        Commands::SplitBenchmark {
            fixtures,
            sweep,
            guardrails_out,
            profile_out,
        } => {
            use benchmark_harness::split_benchmark::{
                SplitBenchmarkConfig, print_split_table, print_sweep_table, run_split_benchmark, run_threshold_sweep,
            };

            let config = SplitBenchmarkConfig {
                fixtures_dir: fixtures,
                sweep,
                guardrails_out,
            };

            let results = if let Some(ref svg_path) = profile_out {
                use benchmark_harness::profiling::ProfileGuard;
                if let Some(parent) = svg_path.parent() {
                    std::fs::create_dir_all(parent).map_err(benchmark_harness::Error::Io)?;
                }
                let guard = ProfileGuard::new(1000)?;
                let results = run_split_benchmark(&config).await?;
                let profiling_result = guard.finish()?;
                profiling_result.generate_flamegraph(svg_path)?;
                tracing::info!(output = %svg_path.display(), "flamegraph written");
                results
            } else {
                run_split_benchmark(&config).await?
            };

            print_split_table(&results);

            if sweep {
                let cells = run_threshold_sweep(&config).await?;
                print_sweep_table(&cells);
            }

            Ok(())
        }

        Commands::EmbedBenchmark => {
            benchmark_harness::embed_benchmark::run_embed_benchmark();
            Ok(())
        }

        Commands::ValidateGt { fixtures, fix, strict } => {
            use benchmark_harness::validate_gt::{ValidateGtConfig, validate_ground_truth};

            let config = ValidateGtConfig {
                fixtures_dir: fixtures,
                fix,
                strict,
            };

            let report = validate_ground_truth(&config)?;

            println!("=== Ground Truth Validation Report ===\n");
            println!("Total fixtures:       {}", report.total_fixtures);
            println!("With text GT:         {}", report.with_text_gt);
            println!("With markdown GT:     {}", report.with_markdown_gt);
            println!("Missing text GT:      {}", report.missing_text_gt);
            println!("Missing markdown GT:  {}", report.missing_markdown_gt);

            if !report.small_gt_files.is_empty() {
                println!("\nSmall GT files (<10 bytes):");
                for (path, size) in &report.small_gt_files {
                    println!("  {} ({} bytes)", path, size);
                }
            }

            if !report.html_issues.is_empty() {
                println!("\nHTML issues in markdown GT ({} file(s)):", report.html_issues.len());
                for (path, tags) in &report.html_issues {
                    let preview: Vec<&str> = tags.iter().take(3).map(|s| s.as_str()).collect();
                    let suffix = if tags.len() > 3 {
                        format!(" (and {} more)", tags.len() - 3)
                    } else {
                        String::new()
                    };
                    println!("  {} - {} tag(s): {}{}", path, tags.len(), preview.join(", "), suffix);
                }
            }

            if !report.noisy_gt_files.is_empty() {
                println!(
                    "\nNoisy GT files ({} file(s) with Warning/Error noise issues):",
                    report.noisy_gt_files.len()
                );
                for (path, count) in &report.noisy_gt_files {
                    println!("  {} ({} issue(s))", path, count);
                }
            }

            if !report.low_diversity_gt.is_empty() {
                println!(
                    "\nLow diversity GT files ({} file(s) with no headings for >100 byte docs):",
                    report.low_diversity_gt.len()
                );
                for path in &report.low_diversity_gt {
                    println!("  {}", path);
                }
            }

            if fix && report.fixes_applied > 0 {
                println!("\nFixes applied: {}", report.fixes_applied);
            }

            if !report.load_failures.is_empty() {
                println!(
                    "\nFixtures that failed to load ({} — e.g. missing/unreadable ground truth):",
                    report.load_failures.len()
                );
                for (path, error) in report.load_failures.iter().take(10) {
                    println!("  {path}: {error}");
                }
                if report.load_failures.len() > 10 {
                    println!("  ... and {} more", report.load_failures.len() - 10);
                }
            }

            if report.html_issues.is_empty()
                && report.small_gt_files.is_empty()
                && report.noisy_gt_files.is_empty()
                && report.low_diversity_gt.is_empty()
                && report.load_failures.is_empty()
            {
                println!("\nAll ground truth files are valid.");
            }

            if strict && !report.load_failures.is_empty() {
                return Err(benchmark_harness::Error::Benchmark(format!(
                    "{} fixture(s) failed to load their ground truth. If these are reference-corpus \
                     documents, the .corpus-cache was not restored (run restore-corpus-cache.sh).",
                    report.load_failures.len()
                )));
            }

            Ok(())
        }

        Commands::FieldQuality {
            fixtures,
            mode,
            dataset,
            filter,
        } => {
            use benchmark_harness::field_quality::{Args, Mode, run};

            let parsed_mode = Mode::parse(&mode).ok_or_else(|| {
                benchmark_harness::Error::Config(format!(
                    "Invalid mode '{}': expected form-fields, formula, or structured",
                    mode
                ))
            })?;

            let args = Args {
                fixtures,
                mode: parsed_mode,
                dataset,
                filter,
            };

            run(args)
                .await
                .map_err(|e| benchmark_harness::Error::Benchmark(format!("{e:#}")))
        }

        Commands::ScoreOutputs {
            fixtures,
            outputs,
            output,
        } => {
            let report = benchmark_harness::score_outputs::score_outputs(&fixtures, &outputs)?;
            let json = serde_json::to_string_pretty(&report).map_err(benchmark_harness::Error::Json)?;
            if let Some(path) = output {
                if let Some(parent) = path.parent()
                    && !parent.as_os_str().is_empty()
                {
                    std::fs::create_dir_all(parent).map_err(benchmark_harness::Error::Io)?;
                }
                std::fs::write(&path, json).map_err(benchmark_harness::Error::Io)?;
                tracing::info!(
                    document_count = report.document_count,
                    mean_sf1 = report.mean_sf1,
                    mean_tf1 = report.mean_tf1,
                    output = %path.display(),
                    "outputs scored"
                );
            } else {
                println!("{json}");
            }
            Ok(())
        }

        Commands::MeasureFrameworkSizes { output } => {
            use benchmark_harness::{measure_framework_sizes, save_framework_sizes};

            println!("Measuring framework installation sizes...");

            let sizes = measure_framework_sizes()?;

            println!("\nFramework sizes:");
            let mut items: Vec<_> = sizes.iter().collect();
            items.sort_by_key(|(k, _)| *k);

            for (name, info) in &items {
                let size_str = if info.size_bytes > 0 {
                    format_size(info.size_bytes)
                } else {
                    "unknown".to_string()
                };
                let status = "";
                let sys_str = if info.system_deps_bytes > 0 {
                    format!(
                        " (pkg: {}, sys: {})",
                        format_size(info.package_bytes),
                        format_size(info.system_deps_bytes)
                    )
                } else {
                    String::new()
                };
                println!("  {}: {}{}{} - {}", name, size_str, sys_str, status, info.description);
            }

            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(benchmark_harness::Error::Io)?;
            }

            save_framework_sizes(&sizes, &output)?;
            println!("\nSizes written to: {}", output.display());

            Ok(())
        }

        Commands::GapReport {
            results,
            output,
            baseline,
            layout,
            competitors,
        } => {
            let output_dir = output.unwrap_or_else(|| results.clone());
            let config = benchmark_harness::gap_report::GapConfig {
                baseline,
                layout,
                competitors,
            };
            benchmark_harness::gap_report::generate(&results, &output_dir, &config)?;
            Ok(())
        }
    }
}

fn tracing_filter(rust_log: Option<&str>, benchmark_debug: bool) -> tracing_subscriber::EnvFilter {
    const DEFAULT_FILTER: &str = "benchmark_harness=info";
    const DEBUG_FILTER: &str = "benchmark_harness=debug";

    rust_log
        .and_then(|filter| tracing_subscriber::EnvFilter::try_new(filter).ok())
        .unwrap_or_else(|| {
            tracing_subscriber::EnvFilter::new(if benchmark_debug { DEBUG_FILTER } else { DEFAULT_FILTER })
        })
}

fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} bytes", bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{Cli, Commands, normalize_run_frameworks, parse_model_provenance, tracing_filter};
    use clap::Parser;

    #[test]
    fn tracing_filter_keeps_status_events_visible_by_default() {
        assert_eq!(tracing_filter(None, false).to_string(), "benchmark_harness=info");
    }

    #[test]
    fn tracing_filter_enables_debug_events_for_benchmark_debug() {
        assert_eq!(tracing_filter(None, true).to_string(), "benchmark_harness=debug");
    }

    #[test]
    fn tracing_filter_honors_explicit_rust_log() {
        assert_eq!(
            tracing_filter(Some("benchmark_harness=trace"), false).to_string(),
            "benchmark_harness=trace"
        );
    }

    #[test]
    fn batch_mode_normalizes_unsuffixed_xberg_aliases() {
        let names = normalize_run_frameworks(
            &[
                "xberg-markdown-baseline".to_string(),
                "xberg-markdown-layout".to_string(),
                "liteparse".to_string(),
            ],
            true,
        );
        assert_eq!(
            names,
            [
                "xberg-markdown-baseline-batch",
                "xberg-markdown-layout-batch",
                "liteparse"
            ]
        );
    }

    #[test]
    fn batch_mode_preserves_and_deduplicates_canonical_names() {
        let names = normalize_run_frameworks(
            &[
                "xberg-markdown-baseline-batch".to_string(),
                "xberg-markdown-baseline".to_string(),
            ],
            true,
        );
        assert_eq!(names, ["xberg-markdown-baseline-batch"]);
    }

    #[test]
    fn single_mode_preserves_xberg_names() {
        let names = normalize_run_frameworks(&["xberg-markdown-baseline".to_string()], false);
        assert_eq!(names, ["xberg-markdown-baseline"]);
    }

    #[test]
    fn score_outputs_cli_accepts_required_paths() {
        let cli = Cli::try_parse_from([
            "benchmark-harness",
            "score-outputs",
            "--fixtures",
            "fixtures",
            "--outputs",
            "outputs",
        ])
        .unwrap();

        assert!(matches!(cli.command, Commands::ScoreOutputs { .. }));
    }

    #[test]
    fn run_cli_accepts_exact_cohort_and_model_identity() {
        let cli = Cli::try_parse_from([
            "benchmark-harness",
            "run",
            "--fixtures",
            "fixtures",
            "--cohort",
            "cohorts/fast.json",
            "--batch-size",
            "4",
            "--xberg-max-threads",
            "8",
            "--model-id",
            "docling=ds4sd/docling-models@main#abc123",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Commands::Run {
                batch_size: Some(4),
                xberg_max_threads: Some(8),
                ..
            }
        ));
        assert!(parse_model_provenance(&["invalid".to_string()]).is_err());
    }

    #[test]
    fn run_help_documents_cohort_path_precedence() {
        let Err(error) = Cli::try_parse_from(["benchmark-harness", "run", "--help"]) else {
            panic!("--help should exit through clap");
        };
        let help = error.to_string();

        assert!(help.contains("Absolute paths are used directly"));
        assert!(help.contains("existing relative paths use the current directory"));
        assert!(help.contains("others use the fixture directory"));
    }
}
