//! 6-path pipeline benchmark: exhaustive quality + timing comparison across
//! all extraction configurations on the full document corpus.
//!
//! | ID | Name              | Config                                           |
//! |----|-------------------|--------------------------------------------------|
//! | P1 | native            | output_format: Markdown                          |
//! | P2 | native+layout     | output_format: Markdown, layout: fast             |
//! | P3 | tesseract         | output_format: Markdown, ocr: tesseract, force    |
//! | P4 | tesseract+layout  | P3 + layout: fast                                |
//! | P5 | paddleocr         | output_format: Markdown, ocr: paddleocr, force (mobile default) |
//! | P6 | paddleocr+layout  | P5 + layout: accurate                            |
//! | P7 | paddleocr-server  | P5 + model_tier: server                           |
//! | P8 | paddleocr-server+layout | P7 + layout: accurate                       |

use crate::Result;
use crate::comparison::{Pipeline, PipelineResult};
use crate::corpus::{self, CorpusDocument, CorpusFilter};
use crate::quality::structural_sidecar::{self, StructuralNode, StructuralSidecar};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Which pipeline paths to include.
pub struct PipelineBenchmarkConfig {
    pub fixtures_dir: PathBuf,
    pub paths: Vec<Pipeline>,
    pub doc_filter: Vec<String>,
    /// Exact fixture stems resolved from a maintained benchmark group.
    pub exact_doc_filter: Vec<String>,
    pub dump_outputs: bool,
    pub json_output: Option<PathBuf>,
    pub sort_by: SortMetric,
    pub bottom_n: Option<usize>,
    pub triage_blocks: bool,
}

/// Metric to sort by in triage view.
#[derive(Debug, Clone, Copy, Default)]
pub enum SortMetric {
    #[default]
    Sf1,
    Tf1,
    Time,
}

impl SortMetric {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "sf1" => Some(SortMetric::Sf1),
            "tf1" => Some(SortMetric::Tf1),
            "time" => Some(SortMetric::Time),
            _ => None,
        }
    }

    fn extract(&self, pr: &PipelineResult) -> f64 {
        match self {
            SortMetric::Sf1 => pr.sf1,
            SortMetric::Tf1 => pr.tf1,
            SortMetric::Time => {
                if pr.time_ms.is_nan() {
                    f64::NEG_INFINITY
                } else {
                    -pr.time_ms
                }
            }
        }
    }
}

/// Result for one document across all selected pipeline paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDocResult {
    pub name: String,
    pub file_type: String,
    pub file_size: u64,
    pub results: Vec<PipelineResult>,
}

/// Per-pipeline aggregate statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineAggregate {
    pub pipeline: String,
    pub mean_sf1: f64,
    pub mean_tf1: f64,
    pub mean_time_ms: f64,
    pub p50_sf1: f64,
    pub p50_tf1: f64,
    pub p50_time_ms: f64,
    pub p90_time_ms: f64,
}

/// Full benchmark run summary for JSON serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRunSummary {
    pub timestamp: String,
    pub git_sha: String,
    pub doc_count: usize,
    pub pipeline_count: usize,
    pub aggregates: Vec<PipelineAggregate>,
    pub docs: Vec<PipelineDocResult>,
    #[serde(default)]
    pub provenance: PipelineRunProvenance,
}

/// Inputs required to reproduce and audit a benchmark result.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PipelineRunProvenance {
    pub hash_algorithm: String,
    pub git_dirty: bool,
    pub git_diff_hash: Option<String>,
    pub binary_hash: Option<String>,
    pub scorer_hash: String,
    pub corpus_hash: Option<String>,
    pub config_hash: Option<String>,
    pub features: Vec<String>,
    pub argv: Vec<String>,
}

/// Default 6-path set.
pub fn default_paths() -> Vec<Pipeline> {
    vec![
        Pipeline::Baseline,
        Pipeline::Layout,
        Pipeline::Tesseract,
        Pipeline::TesseractLayout,
        Pipeline::Paddle,
        Pipeline::PaddleLayout,
    ]
}

/// Opt-in candle OCR suite — evaluates the candle backends against the
/// baseline + layout paths on OCR-relevant fixtures. Used for Phase 6 model
/// selection.
pub fn candle_ocr_suite() -> Vec<Pipeline> {
    vec![
        Pipeline::Baseline,
        Pipeline::Layout,
        Pipeline::CandleTrocr,
        Pipeline::CandlePaddleocrVl,
    ]
}

async fn extract_and_score(
    pipeline: Pipeline,
    doc: &CorpusDocument,
    gt_text: &str,
    gt_markdown: Option<&str>,
    fixtures_dir: &Path,
) -> PipelineResult {
    let (content_opt, time_ms) = crate::comparison::extract_pipeline(pipeline, doc, fixtures_dir).await;
    let extraction_failed = content_opt.is_none();
    let content = content_opt.unwrap_or_default();
    let tf1 = if extraction_failed {
        f64::NAN
    } else {
        crate::comparison::score_document(&content, gt_text, gt_markdown).0
    };

    let structural = match (extraction_failed, gt_markdown) {
        (false, Some(md)) => score_structural_markdown(&content, md),
        _ => crate::comparison::StructuralBreakdown {
            sf1: f64::NAN,
            order_score: f64::NAN,
            ..Default::default()
        },
    };

    let char_similarity = if extraction_failed {
        f64::NAN
    } else {
        crate::quality::normalized_edit_similarity(&content, gt_text)
    };

    let ext_tokens = crate::quality::tokenize(&content);
    let gt_tok = crate::quality::tokenize(gt_text);
    let (mut missing_tokens, mut extra_tokens) = crate::quality::compute_token_diff(&ext_tokens, &gt_tok);
    missing_tokens.truncate(50);
    extra_tokens.truncate(50);

    PipelineResult {
        pipeline,
        sf1: structural.sf1,
        tf1,
        char_similarity,
        order_score: structural.order_score,
        per_type_sf1: structural.per_type_sf1,
        per_type_precision: structural.per_type_precision,
        per_type_recall: structural.per_type_recall,
        time_ms,
        missing_tokens,
        extra_tokens,
        content,
    }
}

fn score_structural_markdown(predicted: &str, ground_truth: &str) -> crate::comparison::StructuralBreakdown {
    use crate::comparison::StructuralBreakdown;

    let gt_sidecar = StructuralSidecar::from_markdown(ground_truth);
    let has_structure = gt_sidecar
        .nodes
        .iter()
        .any(|node| !matches!(node, StructuralNode::Paragraph { .. }));

    if !has_structure {
        return StructuralBreakdown {
            sf1: f64::NAN,
            order_score: f64::NAN,
            ..Default::default()
        };
    }

    let score = structural_sidecar::score_structural(&StructuralSidecar::from_markdown(predicted), &gt_sidecar);
    StructuralBreakdown::from_score(&score)
}

/// Run the pipeline benchmark.
pub async fn run_pipeline_benchmark(config: &PipelineBenchmarkConfig) -> Result<Vec<PipelineDocResult>> {
    let filter = CorpusFilter {
        file_types: None,
        require_ground_truth: true,
        name_patterns: config.doc_filter.clone(),
        exact_names: config.exact_doc_filter.clone(),
        ..Default::default()
    };

    let docs = corpus::build_corpus(&config.fixtures_dir, &filter)?;
    if docs.is_empty() {
        return Err(crate::Error::Config(
            "pipeline benchmark selection matched zero documents".to_string(),
        ));
    }
    eprintln!(
        "Pipeline benchmark: {} documents, {} paths",
        docs.len(),
        config.paths.len()
    );

    let dump_dir = if config.dump_outputs {
        let dir = PathBuf::from("/tmp/xberg_pipeline");
        let _ = std::fs::create_dir_all(&dir);
        Some(dir)
    } else {
        None
    };

    let mut results = Vec::new();
    let total = docs.len();

    for (idx, doc) in docs.iter().enumerate() {
        eprint!("\r[{}/{}] {} ...", idx + 1, total, doc.name);
        let gt_text = match doc.ground_truth_text.as_ref() {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Warning: failed to read ground truth text {}: {}", p.display(), e);
                    String::new()
                }
            },
            None => String::new(),
        };
        let gt_markdown = match doc.ground_truth_markdown.as_ref() {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("Warning: failed to read ground truth markdown {}: {}", p.display(), e);
                    None
                }
            },
            None => None,
        };

        let mut pipeline_results = Vec::new();

        for &pipeline in &config.paths {
            let pr = extract_and_score(pipeline, doc, &gt_text, gt_markdown.as_deref(), &config.fixtures_dir).await;

            if let Some(ref dir) = dump_dir {
                let doc_dir = dir.join(&doc.name);
                let _ = std::fs::create_dir_all(&doc_dir);
                let _ = std::fs::write(doc_dir.join(format!("{}.md", pipeline.name())), &pr.content);
                if let Some(ref gt_md) = gt_markdown {
                    let _ = std::fs::write(doc_dir.join("ground_truth.md"), gt_md);
                }
                let _ = std::fs::write(doc_dir.join("ground_truth_text.txt"), &gt_text);
            }

            pipeline_results.push(pr);
        }

        let best_sf1 = pipeline_results.iter().map(|r| r.sf1).fold(0.0_f64, f64::max);
        let best_time = pipeline_results
            .iter()
            .map(|r| r.time_ms)
            .filter(|t| !t.is_nan())
            .fold(f64::INFINITY, f64::min);
        if best_time.is_infinite() {
            eprint!(
                "\r[{}/{}] {:<30} SF1:{:.0}%\n",
                idx + 1,
                total,
                doc.name,
                best_sf1 * 100.0,
            );
        } else {
            eprint!(
                "\r[{}/{}] {:<30} SF1:{:.0}% {:.0}ms\n",
                idx + 1,
                total,
                doc.name,
                best_sf1 * 100.0,
                best_time
            );
        }

        results.push(PipelineDocResult {
            name: doc.name.clone(),
            file_type: doc.file_type.clone(),
            file_size: doc.file_size,
            results: pipeline_results,
        });
    }

    Ok(results)
}

/// Print a per-document + aggregate matrix table.
pub fn print_pipeline_table(results: &[PipelineDocResult], sort_by: SortMetric, bottom_n: Option<usize>) {
    if results.is_empty() {
        eprintln!("No results.");
        return;
    }

    let display_results: Vec<&PipelineDocResult> = if let Some(n) = bottom_n {
        let mut sorted: Vec<&PipelineDocResult> = results.iter().collect();
        sorted.sort_by(|a, b| {
            let a_worst = a
                .results
                .iter()
                .map(|pr| sort_by.extract(pr))
                .fold(f64::INFINITY, f64::min);
            let b_worst = b
                .results
                .iter()
                .map(|pr| sort_by.extract(pr))
                .fold(f64::INFINITY, f64::min);
            a_worst.partial_cmp(&b_worst).unwrap_or(std::cmp::Ordering::Equal)
        });
        sorted.into_iter().take(n).collect()
    } else {
        results.iter().collect()
    };

    let pipelines: Vec<&str> = results[0].results.iter().map(|r| r.pipeline.name()).collect();

    eprint!("{:<30} {:>5}", "Document", "Type");
    for p in &pipelines {
        eprint!(" {:>8} {:>8} {:>8} {:>7}", format!("{} SF1", p), "TF1", "Ord", "ms");
    }
    eprintln!();
    eprintln!("{}", "-".repeat(36 + pipelines.len() * 35));

    for doc in &display_results {
        eprint!(
            "{:<30} {:>5}",
            if doc.name.len() > 29 {
                &doc.name[..29]
            } else {
                &doc.name
            },
            &doc.file_type,
        );
        for pr in &doc.results {
            let sf1_str = if pr.sf1.is_nan() {
                "    —   ".to_string()
            } else {
                format!("{:>7.1}%", pr.sf1 * 100.0)
            };
            let tf1_str = if pr.tf1.is_nan() {
                "    —   ".to_string()
            } else {
                format!("{:>7.1}%", pr.tf1 * 100.0)
            };
            let ord_str = if pr.order_score.is_nan() {
                "    —   ".to_string()
            } else {
                format!("{:>7.1}%", pr.order_score * 100.0)
            };
            let time_str = if pr.time_ms.is_nan() {
                "    N/A".to_string()
            } else {
                format!("{:>7.0}", pr.time_ms)
            };
            eprint!(" {} {} {} {}", sf1_str, tf1_str, ord_str, time_str);
        }
        eprintln!();
    }

    let total_docs = results.len();
    eprintln!("{}", "-".repeat(36 + pipelines.len() * 35));
    eprint!("{:<30} {:>5}", "AVERAGE", "");
    for (i, _) in pipelines.iter().enumerate() {
        let sf1_vals: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].sf1)
            .filter(|v| v.is_finite())
            .collect();
        let sf1 = if !sf1_vals.is_empty() {
            sf1_vals.iter().sum::<f64>() / sf1_vals.len() as f64
        } else {
            0.0
        };
        let tf1_vals: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].tf1)
            .filter(|v| v.is_finite())
            .collect();
        let tf1 = if !tf1_vals.is_empty() {
            tf1_vals.iter().sum::<f64>() / tf1_vals.len() as f64
        } else {
            0.0
        };
        let order_vals: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].order_score)
            .filter(|v| v.is_finite())
            .collect();
        let order = if !order_vals.is_empty() {
            order_vals.iter().sum::<f64>() / order_vals.len() as f64
        } else {
            0.0
        };
        let time_vals: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].time_ms)
            .filter(|v| v.is_finite())
            .collect();
        if time_vals.is_empty() {
            eprint!(
                " {:>7.1}% {:>7.1}% {:>7.1}% {:>7}",
                sf1 * 100.0,
                tf1 * 100.0,
                order * 100.0,
                "N/A"
            );
        } else {
            let ms: f64 = time_vals.iter().sum::<f64>() / time_vals.len() as f64;
            eprint!(
                " {:>7.1}% {:>7.1}% {:>7.1}% {:>7.0}",
                sf1 * 100.0,
                tf1 * 100.0,
                order * 100.0,
                ms
            );
        }
    }
    eprintln!();
    let sf1_excluded: usize = results.iter().map(|r| r.results[0].sf1).filter(|v| v.is_nan()).count();
    if sf1_excluded > 0 {
        eprintln!(
            "  (SF1 averaged over {}/{} docs; {} paragraph-only docs excluded)",
            total_docs - sf1_excluded,
            total_docs,
            sf1_excluded
        );
    }
}

/// Print per-block-type F1 breakdown for triage.
pub fn print_triage_blocks(results: &[PipelineDocResult], sort_by: SortMetric, bottom_n: usize) {
    if results.is_empty() {
        return;
    }

    const STRUCTURAL_DIMENSIONS: [&str; 7] = [
        "paragraph",
        "heading",
        "list",
        "table",
        "table_content",
        "edges",
        "order",
    ];

    let mut sorted: Vec<&PipelineDocResult> = results.iter().collect();
    sorted.sort_by(|a, b| {
        let a_worst = a
            .results
            .iter()
            .map(|pr| sort_by.extract(pr))
            .fold(f64::INFINITY, f64::min);
        let b_worst = b
            .results
            .iter()
            .map(|pr| sort_by.extract(pr))
            .fold(f64::INFINITY, f64::min);
        a_worst.partial_cmp(&b_worst).unwrap_or(std::cmp::Ordering::Equal)
    });
    let display: Vec<&PipelineDocResult> = sorted.into_iter().take(bottom_n).collect();

    eprintln!("\nPer-block-type F1 breakdown (bottom {} documents):", bottom_n);

    for doc in &display {
        eprintln!("\n  {}", doc.name);
        for pr in &doc.results {
            let blocks_str: String = STRUCTURAL_DIMENSIONS
                .iter()
                .filter_map(|bt| {
                    pr.per_type_sf1.get(*bt).map(|f1| {
                        // Show the precision/recall split when present so a low F1
                        // reads as fabrication (low p) vs omission (low r).
                        match (pr.per_type_precision.get(*bt), pr.per_type_recall.get(*bt)) {
                            (Some(p), Some(r)) => {
                                format!("{}:{:.0}%(p{:.0}/r{:.0})", bt, f1 * 100.0, p * 100.0, r * 100.0)
                            }
                            _ => format!("{}:{:.0}%", bt, f1 * 100.0),
                        }
                    })
                })
                .collect::<Vec<_>>()
                .join("  ");
            let sim_str = if pr.char_similarity.is_nan() {
                String::new()
            } else {
                format!("  Sim:{:.0}%", pr.char_similarity * 100.0)
            };
            eprintln!(
                "    {:<18} SF1:{:.0}%  TF1:{:.0}%{}  {}",
                pr.pipeline.name(),
                pr.sf1 * 100.0,
                pr.tf1 * 100.0,
                sim_str,
                blocks_str
            );
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (p * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Compute per-pipeline aggregate statistics.
pub fn compute_aggregates(results: &[PipelineDocResult]) -> Vec<PipelineAggregate> {
    if results.is_empty() {
        return Vec::new();
    }

    let num_pipelines = results[0].results.len();
    let mut aggregates = Vec::new();

    for i in 0..num_pipelines {
        let pipeline_name = results[0].results[i].pipeline.name().to_string();

        let mut sf1s: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].sf1)
            .filter(|v| v.is_finite())
            .collect();
        let mut tf1s: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].tf1)
            .filter(|v| v.is_finite())
            .collect();
        let mut times: Vec<f64> = results
            .iter()
            .map(|r| r.results[i].time_ms)
            .filter(|v| v.is_finite())
            .collect();

        sf1s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        tf1s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let sf1_n = sf1s.len() as f64;
        let tf1_n = tf1s.len() as f64;

        aggregates.push(PipelineAggregate {
            pipeline: pipeline_name,
            mean_sf1: if sf1_n > 0.0 {
                sf1s.iter().sum::<f64>() / sf1_n
            } else {
                f64::NAN
            },
            mean_tf1: if tf1_n > 0.0 {
                tf1s.iter().sum::<f64>() / tf1_n
            } else {
                f64::NAN
            },
            mean_time_ms: if times.is_empty() {
                f64::NAN
            } else {
                times.iter().sum::<f64>() / times.len() as f64
            },
            p50_sf1: if sf1s.is_empty() {
                f64::NAN
            } else {
                percentile(&sf1s, 0.5)
            },
            p50_tf1: if tf1s.is_empty() {
                f64::NAN
            } else {
                percentile(&tf1s, 0.5)
            },
            p50_time_ms: percentile(&times, 0.5),
            p90_time_ms: percentile(&times, 0.9),
        });
    }

    aggregates
}

/// Build a full run summary for JSON serialization.
pub fn build_summary(results: &[PipelineDocResult]) -> PipelineRunSummary {
    build_summary_with_config(results, None)
}

/// Build a run summary and fingerprint the exact benchmark inputs when config is available.
pub fn build_summary_with_config(
    results: &[PipelineDocResult],
    config: Option<&PipelineBenchmarkConfig>,
) -> PipelineRunSummary {
    let git_sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    let timestamp = chrono::Utc::now().to_rfc3339();

    PipelineRunSummary {
        timestamp,
        git_sha,
        doc_count: results.len(),
        pipeline_count: results.first().map(|r| r.results.len()).unwrap_or(0),
        aggregates: compute_aggregates(results),
        docs: results.to_vec(),
        provenance: build_provenance(config),
    }
}

fn build_provenance(config: Option<&PipelineBenchmarkConfig>) -> PipelineRunProvenance {
    let status = command_output("git", &["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    let git_dirty = status.as_ref().is_some_and(|bytes| !bytes.is_empty());
    let git_diff_hash = git_dirty.then(hash_worktree).flatten();

    PipelineRunProvenance {
        hash_algorithm: "blake3".to_string(),
        git_dirty,
        git_diff_hash,
        binary_hash: std::env::current_exe()
            .ok()
            .and_then(|path| hash_files(&[("binary", path)])),
        scorer_hash: scorer_hash(),
        corpus_hash: config.and_then(hash_selected_corpus),
        config_hash: config.map(hash_config),
        features: enabled_features(),
        argv: std::env::args_os()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect(),
    }
}

fn command_output(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    let output = std::process::Command::new(program).args(args).output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn hash_worktree() -> Option<String> {
    let mut hasher = blake3::Hasher::new();
    hash_bytes_into(
        &mut hasher,
        &command_output("git", &["diff", "--binary", "HEAD", "--"])?,
    );
    let untracked = command_output("git", &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut paths: Vec<&[u8]> = untracked
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort_unstable();
    for path in paths {
        hash_bytes_into(&mut hasher, path);
        let path = PathBuf::from(String::from_utf8_lossy(path).into_owned());
        hash_file_into(&mut hasher, &path).ok()?;
    }
    Some(hasher.finalize().to_hex().to_string())
}

fn scorer_hash() -> String {
    let mut hasher = blake3::Hasher::new();
    for source in [
        include_bytes!("structural_sidecar.rs").as_slice(),
        include_bytes!("quality.rs").as_slice(),
        include_bytes!("markdown_quality.rs").as_slice(),
        include_bytes!("pipeline_benchmark.rs").as_slice(),
    ] {
        hash_bytes_into(&mut hasher, source);
    }
    hasher.finalize().to_hex().to_string()
}

fn hash_config(config: &PipelineBenchmarkConfig) -> String {
    let mut hasher = blake3::Hasher::new();
    hash_bytes_into(&mut hasher, config.fixtures_dir.to_string_lossy().as_bytes());
    for path in &config.paths {
        hash_bytes_into(&mut hasher, path.name().as_bytes());
    }
    for pattern in &config.doc_filter {
        hash_bytes_into(&mut hasher, pattern.as_bytes());
    }
    for name in &config.exact_doc_filter {
        hash_bytes_into(&mut hasher, name.as_bytes());
    }
    hash_bytes_into(&mut hasher, &[u8::from(config.dump_outputs)]);
    match &config.json_output {
        Some(path) => hash_bytes_into(&mut hasher, path.to_string_lossy().as_bytes()),
        None => hash_bytes_into(&mut hasher, &[]),
    }
    hash_bytes_into(&mut hasher, format!("{:?}", config.sort_by).as_bytes());
    hash_bytes_into(&mut hasher, &config.bottom_n.unwrap_or(usize::MAX).to_le_bytes());
    hash_bytes_into(&mut hasher, &[u8::from(config.triage_blocks)]);
    hasher.finalize().to_hex().to_string()
}

fn hash_selected_corpus(config: &PipelineBenchmarkConfig) -> Option<String> {
    let docs = corpus::build_corpus(
        &config.fixtures_dir,
        &CorpusFilter {
            require_ground_truth: true,
            name_patterns: config.doc_filter.clone(),
            exact_names: config.exact_doc_filter.clone(),
            ..Default::default()
        },
    )
    .ok()?;
    let mut files = Vec::new();
    for doc in docs {
        files.push((format!("{}/fixture", doc.name), doc.fixture_path));
        files.push((format!("{}/document", doc.name), doc.document_path));
        if let Some(path) = doc.ground_truth_text {
            files.push((format!("{}/ground_truth_text", doc.name), path));
        }
        if let Some(path) = doc.ground_truth_markdown {
            files.push((format!("{}/ground_truth_markdown", doc.name), path));
        }
    }
    let refs: Vec<(&str, PathBuf)> = files
        .iter()
        .map(|(label, path)| (label.as_str(), path.clone()))
        .collect();
    hash_files(&refs)
}

fn hash_files(files: &[(&str, PathBuf)]) -> Option<String> {
    let mut hasher = blake3::Hasher::new();
    for (label, path) in files {
        hash_bytes_into(&mut hasher, label.as_bytes());
        hash_file_into(&mut hasher, path).ok()?;
    }
    Some(hasher.finalize().to_hex().to_string())
}

fn hash_file_into(hasher: &mut blake3::Hasher, path: &Path) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    hasher.update(&file.metadata()?.len().to_le_bytes());
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        hasher.update(&buffer[..read]);
    }
}

fn hash_bytes_into(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn enabled_features() -> Vec<String> {
    let mut features = vec!["xberg/full".to_string()];
    for (enabled, name) in [
        (cfg!(feature = "profiling"), "profiling"),
        (cfg!(feature = "memory-profiling"), "memory-profiling"),
        (cfg!(feature = "glm-ocr-bench"), "glm-ocr-bench"),
        (cfg!(feature = "candle-deepseek-ocr-bench"), "candle-deepseek-ocr-bench"),
        (
            cfg!(feature = "candle-paddleocr-vl-15-bench"),
            "candle-paddleocr-vl-15-bench",
        ),
    ] {
        if enabled {
            features.push(name.to_string());
        }
    }
    features
}

/// Write the run summary to a JSON file.
pub fn write_json_output(results: &[PipelineDocResult], path: &std::path::Path) -> Result<()> {
    let summary = build_summary(results);
    write_summary(&summary, path)
}

/// Write a run summary including hashes for the selected config and corpus.
pub fn write_json_output_with_config(
    results: &[PipelineDocResult],
    path: &std::path::Path,
    config: &PipelineBenchmarkConfig,
) -> Result<()> {
    let summary = build_summary_with_config(results, Some(config));
    write_summary(&summary, path)
}

fn write_summary(summary: &PipelineRunSummary, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(crate::Error::Io)?;
    }
    let json = serde_json::to_string_pretty(&summary)
        .map_err(|e| crate::Error::Benchmark(format!("Failed to serialize: {}", e)))?;
    std::fs::write(path, json).map_err(crate::Error::Io)?;
    eprintln!("JSON output written to: {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn test_config(fixtures_dir: PathBuf) -> PipelineBenchmarkConfig {
        PipelineBenchmarkConfig {
            fixtures_dir,
            paths: vec![Pipeline::Baseline],
            doc_filter: Vec::new(),
            exact_doc_filter: Vec::new(),
            dump_outputs: false,
            json_output: None,
            sort_by: SortMetric::Sf1,
            bottom_n: None,
            triage_blocks: false,
        }
    }

    #[test]
    fn structural_scoring_uses_content_after_50_kib() {
        let prefix = format!("{}\n\n", "plain text ".repeat(6_000));
        assert!(prefix.len() > 50 * 1024);
        let markdown = format!("{prefix}# Tail heading\n");
        let structural = score_structural_markdown(&markdown, &markdown);
        assert_eq!(structural.sf1, 1.0);
        assert_eq!(structural.per_type_sf1.get("heading"), Some(&1.0));
    }

    #[test]
    fn aggregates_ignore_unavailable_tf1_scores() {
        let pipeline_result = |tf1| PipelineResult {
            pipeline: Pipeline::Docling,
            sf1: tf1,
            tf1,
            char_similarity: tf1,
            order_score: tf1,
            per_type_sf1: HashMap::new(),
            per_type_precision: HashMap::new(),
            per_type_recall: HashMap::new(),
            time_ms: 10.0,
            missing_tokens: Vec::new(),
            extra_tokens: Vec::new(),
            content: String::new(),
        };
        let doc_result = |name: &str, tf1| PipelineDocResult {
            name: name.to_string(),
            file_type: "pdf".to_string(),
            file_size: 1,
            results: vec![pipeline_result(tf1)],
        };
        let results = vec![doc_result("success", 0.75), doc_result("failure", f64::NAN)];

        let aggregates = compute_aggregates(&results);

        assert_eq!(aggregates.len(), 1);
        assert_eq!(aggregates[0].mean_tf1, 0.75);
        assert_eq!(aggregates[0].p50_tf1, 0.75);
        assert!(
            serde_json::to_string(&aggregates)
                .unwrap()
                .contains("\"mean_tf1\":0.75")
        );
    }

    #[test]
    fn aggregates_preserve_all_unavailable_scores() {
        let results = vec![PipelineDocResult {
            name: "failure".to_string(),
            file_type: "pdf".to_string(),
            file_size: 1,
            results: vec![PipelineResult {
                pipeline: Pipeline::Docling,
                sf1: f64::NAN,
                tf1: f64::NAN,
                char_similarity: f64::NAN,
                order_score: f64::NAN,
                per_type_sf1: HashMap::new(),
                per_type_precision: HashMap::new(),
                per_type_recall: HashMap::new(),
                time_ms: f64::NAN,
                missing_tokens: Vec::new(),
                extra_tokens: Vec::new(),
                content: String::new(),
            }],
        }];

        let aggregates = compute_aggregates(&results);
        let serialized = serde_json::to_value(&aggregates).unwrap();

        assert!(aggregates[0].mean_sf1.is_nan());
        assert!(aggregates[0].mean_tf1.is_nan());
        assert!(aggregates[0].p50_sf1.is_nan());
        assert!(aggregates[0].p50_tf1.is_nan());
        assert!(serialized[0]["mean_sf1"].is_null());
        assert!(serialized[0]["mean_tf1"].is_null());
        assert!(serialized[0]["p50_sf1"].is_null());
        assert!(serialized[0]["p50_tf1"].is_null());
    }

    #[test]
    fn legacy_summary_deserializes_without_provenance() {
        let summary: PipelineRunSummary = serde_json::from_value(serde_json::json!({
            "timestamp": "2026-01-01T00:00:00Z",
            "git_sha": "abc",
            "doc_count": 0,
            "pipeline_count": 0,
            "aggregates": [],
            "docs": []
        }))
        .unwrap();
        assert!(summary.provenance.hash_algorithm.is_empty());
    }

    #[test]
    fn config_hash_tracks_exact_selection() {
        let mut config = test_config(PathBuf::from("fixtures"));
        let before = hash_config(&config);
        config.exact_doc_filter.push("fixture-a".to_string());
        assert_ne!(before, hash_config(&config));
    }

    #[tokio::test]
    async fn empty_selection_is_an_error() {
        let fixtures = tempfile::tempdir().unwrap();
        let error = run_pipeline_benchmark(&test_config(fixtures.path().to_path_buf()))
            .await
            .unwrap_err();
        assert!(matches!(error, crate::Error::Config(_)));
    }
}
