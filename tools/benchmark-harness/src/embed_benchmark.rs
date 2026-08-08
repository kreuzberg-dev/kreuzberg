//! Embedding benchmark: throughput, latency, and batch-size sweep across presets.
//!
//! Measures embedding generation performance for each preset (fast, balanced,
//! quality, multilingual) including:
//! - Model warm-up latency (first-call overhead: download + ONNX init)
//! - Steady-state throughput: chunks/sec at default batch size
//! - Batch size sweep: throughput at batch sizes 8, 16, 32, 64, 128
//!
//! Requires ONNX Runtime on the system. See `xberg::embeddings` for installation
//! instructions.

use std::time::Instant;

use rayon::prelude::*;

use xberg::embeddings::{EMBEDDING_PRESETS, EmbeddingPreset};
use xberg::{Chunk, ChunkMetadata, EmbeddingConfig, EmbeddingModelType};

/// Embed text content into each chunk using the public `embed_texts` API.
///
/// Mirrors the internal `embed_chunks` behaviour: collects
/// chunk text, calls `embed_texts`, and writes each resulting vector back into
/// `chunk.embedding`.
fn embed_chunks(chunks: &mut [Chunk], config: &EmbeddingConfig) -> xberg::Result<()> {
    if chunks.is_empty() {
        return Ok(());
    }
    let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
    let embeddings = xberg::embed_texts(texts, config)?;
    for (chunk, embedding) in chunks.iter_mut().zip(embeddings) {
        chunk.embedding = Some(embedding);
    }
    Ok(())
}

/// Number of chunks to embed for throughput measurement.
const THROUGHPUT_CHUNK_COUNT: usize = 100;

/// Number of words per chunk used in throughput measurement.
const WORDS_PER_CHUNK: usize = 200;

/// Batch sizes to sweep.
const BATCH_SIZES: &[usize] = &[8, 16, 32, 64, 128];

/// Per-preset benchmark results.
#[derive(Debug)]
pub struct PresetResult {
    pub name: String,
    pub dimensions: usize,
    /// Model warm-up time in milliseconds (first call: download check + ONNX init).
    pub warm_ms: f64,
    /// Total time to embed `THROUGHPUT_CHUNK_COUNT` chunks at default batch size (ms).
    pub total_ms: f64,
    /// Chunks per second at default batch size.
    pub chunks_per_sec: f64,
    /// Milliseconds per chunk at default batch size.
    pub ms_per_chunk: f64,
}

/// Per-batch-size result for the sweep (run on the "balanced" preset).
#[derive(Debug)]
pub struct BatchSweepResult {
    pub batch_size: usize,
    /// Total time to embed `THROUGHPUT_CHUNK_COUNT` chunks (ms).
    pub total_ms: f64,
    pub chunks_per_sec: f64,
    pub ms_per_chunk: f64,
}

/// Parallel inference benchmark result.
#[derive(Debug)]
pub struct ParallelResult {
    pub num_batches: usize,
    pub chunks_per_batch: usize,
    pub total_chunks: usize,
    /// Sequential baseline time in milliseconds.
    pub sequential_ms: f64,
    /// Sequential throughput in chunks per second.
    pub sequential_chunks_per_sec: f64,
    /// Parallel (rayon) time in milliseconds.
    pub parallel_ms: f64,
    /// Parallel throughput in chunks per second.
    pub parallel_chunks_per_sec: f64,
    /// Speedup factor (sequential_ms / parallel_ms).
    pub speedup: f64,
}

/// Full embed benchmark output.
#[derive(Debug)]
pub struct EmbedBenchmarkResults {
    pub presets: Vec<PresetResult>,
    pub batch_sweep: Vec<BatchSweepResult>,
    pub parallel: Option<ParallelResult>,
}

/// Generate synthetic text chunks for benchmarking.
///
/// Each chunk contains `words_per_chunk` space-separated lorem-ipsum-style words
/// to approximate realistic sentence length distributions.
fn generate_test_chunks(count: usize, words_per_chunk: usize) -> Vec<Chunk> {
    const WORDS: &[&str] = &[
        "the",
        "quick",
        "brown",
        "fox",
        "jumps",
        "over",
        "lazy",
        "dog",
        "in",
        "a",
        "field",
        "of",
        "green",
        "grass",
        "under",
        "blue",
        "sky",
        "with",
        "white",
        "clouds",
        "floating",
        "gently",
        "by",
        "as",
        "birds",
        "sing",
        "their",
        "songs",
        "and",
        "children",
        "play",
        "happily",
        "near",
        "river",
        "bank",
        "where",
        "water",
        "flows",
        "crystal",
        "clear",
        "through",
        "ancient",
        "stones",
        "document",
        "extraction",
        "embedding",
        "vector",
        "semantic",
        "search",
        "retrieval",
        "augmented",
        "generation",
        "neural",
        "network",
        "transformer",
        "attention",
        "mechanism",
        "tokenizer",
        "inference",
        "batch",
        "processing",
    ];

    (0..count)
        .map(|i| {
            let text: String = (0..words_per_chunk)
                .map(|j| WORDS[(i * 7 + j * 3) % WORDS.len()])
                .collect::<Vec<_>>()
                .join(" ");
            let byte_end = text.len();

            Chunk {
                content: text,
                embedding: None,
                sparse_embedding: None,
                late_interaction: None,
                chunk_type: Default::default(),
                metadata: ChunkMetadata {
                    byte_start: 0,
                    byte_end,
                    token_count: None,
                    chunk_index: i,
                    total_chunks: count,
                    first_page: None,
                    last_page: None,
                    heading_context: None,
                    heading_path: Vec::new(),
                    image_indices: Vec::new(),
                    node_ids: Vec::new(),
                    page_spans: Vec::new(),
                    classifications: Vec::new(),
                },
            }
        })
        .collect()
}

/// Build an EmbeddingConfig for a given preset at the specified batch size.
fn config_for_preset(preset: &EmbeddingPreset, batch_size: usize) -> EmbeddingConfig {
    EmbeddingConfig {
        model: EmbeddingModelType::Preset {
            name: preset.name.to_string(),
        },
        normalize: true,
        batch_size,
        show_download_progress: false,
        cache_dir: None,
        acceleration: None,
        max_embed_duration_secs: None,
        max_sequence_length: None,
    }
}

/// Run the full embedding benchmark.
///
/// Prints a formatted table to stdout and returns structured results.
pub fn run_embed_benchmark() -> EmbedBenchmarkResults {
    println!("\n=== Embedding Benchmark ===\n");
    println!(
        "Generating {} test chunks (~{} words each)...",
        THROUGHPUT_CHUNK_COUNT, WORDS_PER_CHUNK
    );

    let mut preset_results: Vec<PresetResult> = Vec::new();

    for preset in EMBEDDING_PRESETS.iter() {
        println!(
            "\n[{}] {} dims — {}",
            preset.name, preset.dimensions, preset.description
        );

        let mut warmup_chunks = generate_test_chunks(1, WORDS_PER_CHUNK);
        let warmup_config = config_for_preset(preset, 1);

        print!("  Warming up model...");
        let warm_start = Instant::now();
        match embed_chunks(&mut warmup_chunks, &warmup_config) {
            Ok(()) => {}
            Err(e) => {
                println!(" SKIP ({})", e);
                continue;
            }
        }
        let warm_ms = warm_start.elapsed().as_secs_f64() * 1000.0;
        println!(" {:.0} ms", warm_ms);

        let mut chunks = generate_test_chunks(THROUGHPUT_CHUNK_COUNT, WORDS_PER_CHUNK);
        let throughput_config = config_for_preset(preset, 32);

        print!("  Throughput ({} chunks, batch=32)...", THROUGHPUT_CHUNK_COUNT);
        let t_start = Instant::now();
        match embed_chunks(&mut chunks, &throughput_config) {
            Ok(()) => {}
            Err(e) => {
                println!(" ERROR: {}", e);
                continue;
            }
        }
        let total_ms = t_start.elapsed().as_secs_f64() * 1000.0;
        let chunks_per_sec = THROUGHPUT_CHUNK_COUNT as f64 / (total_ms / 1000.0);
        let ms_per_chunk = total_ms / THROUGHPUT_CHUNK_COUNT as f64;

        println!(
            " {:.1} ms total → {:.1} chunks/sec, {:.2} ms/chunk",
            total_ms, chunks_per_sec, ms_per_chunk
        );

        preset_results.push(PresetResult {
            name: preset.name.clone(),
            dimensions: preset.dimensions,
            warm_ms,
            total_ms,
            chunks_per_sec,
            ms_per_chunk,
        });
    }

    println!(
        "\n--- Batch size sweep (balanced preset, {} chunks) ---\n",
        THROUGHPUT_CHUNK_COUNT
    );

    let balanced = match EMBEDDING_PRESETS.iter().find(|p| p.name == "balanced") {
        Some(p) => p,
        None => {
            eprintln!("WARNING: 'balanced' preset not found; skipping batch sweep.");
            return EmbedBenchmarkResults {
                presets: preset_results,
                batch_sweep: Vec::new(),
                parallel: None,
            };
        }
    };

    let mut sweep_results: Vec<BatchSweepResult> = Vec::new();

    println!(
        "{:>12}  {:>12}  {:>14}  {:>12}",
        "batch_size", "total_ms", "chunks/sec", "ms/chunk"
    );
    println!("{}", "-".repeat(55));

    for &batch_size in BATCH_SIZES {
        let mut chunks = generate_test_chunks(THROUGHPUT_CHUNK_COUNT, WORDS_PER_CHUNK);
        let config = config_for_preset(balanced, batch_size);

        let t_start = Instant::now();
        match embed_chunks(&mut chunks, &config) {
            Ok(()) => {}
            Err(e) => {
                println!("{:>12}  ERROR: {}", batch_size, e);
                continue;
            }
        }
        let total_ms = t_start.elapsed().as_secs_f64() * 1000.0;
        let chunks_per_sec = THROUGHPUT_CHUNK_COUNT as f64 / (total_ms / 1000.0);
        let ms_per_chunk = total_ms / THROUGHPUT_CHUNK_COUNT as f64;

        println!(
            "{:>12}  {:>12.1}  {:>14.1}  {:>12.2}",
            batch_size, total_ms, chunks_per_sec, ms_per_chunk
        );

        sweep_results.push(BatchSweepResult {
            batch_size,
            total_ms,
            chunks_per_sec,
            ms_per_chunk,
        });
    }

    println!("\n--- Parallel inference test (balanced preset) ---\n");

    let parallel_batches: usize = 8;
    let chunks_per_batch: usize = 50;

    let mut batches: Vec<Vec<Chunk>> = (0..parallel_batches)
        .map(|_| generate_test_chunks(chunks_per_batch, WORDS_PER_CHUNK))
        .collect();

    let parallel_config = config_for_preset(balanced, 32);

    let mut seq_batches = batches.clone();
    let seq_start = Instant::now();
    for batch in &mut seq_batches {
        embed_chunks(batch, &parallel_config).expect("Sequential embedding failed");
    }
    let seq_ms = seq_start.elapsed().as_secs_f64() * 1000.0;

    let par_start = Instant::now();
    batches.par_iter_mut().for_each(|batch| {
        embed_chunks(batch, &parallel_config).expect("Parallel embedding failed");
    });
    let par_ms = par_start.elapsed().as_secs_f64() * 1000.0;

    let total_chunks = parallel_batches * chunks_per_batch;
    let speedup = seq_ms / par_ms;
    let seq_chunks_per_sec = total_chunks as f64 / (seq_ms / 1000.0);
    let par_chunks_per_sec = total_chunks as f64 / (par_ms / 1000.0);

    println!(
        "{} batches x {} chunks = {} total chunks",
        parallel_batches, chunks_per_batch, total_chunks
    );
    println!("  Sequential: {:.0} ms ({:.1} chunks/sec)", seq_ms, seq_chunks_per_sec);
    println!("  Parallel:   {:.0} ms ({:.1} chunks/sec)", par_ms, par_chunks_per_sec);
    println!("  Speedup:    {:.2}x", speedup);

    let parallel_result = Some(ParallelResult {
        num_batches: parallel_batches,
        chunks_per_batch,
        total_chunks,
        sequential_ms: seq_ms,
        sequential_chunks_per_sec: seq_chunks_per_sec,
        parallel_ms: par_ms,
        parallel_chunks_per_sec: par_chunks_per_sec,
        speedup,
    });

    if !preset_results.is_empty() {
        println!("\n=== Summary ===\n");
        println!(
            "{:<14}  {:>6}  {:>10}  {:>12}  {:>12}",
            "preset", "dims", "warm_ms", "chunks/sec", "ms/chunk"
        );
        println!("{}", "-".repeat(60));
        for r in &preset_results {
            println!(
                "{:<14}  {:>6}  {:>10.0}  {:>12.1}  {:>12.2}",
                r.name, r.dimensions, r.warm_ms, r.chunks_per_sec, r.ms_per_chunk
            );
        }
    }

    EmbedBenchmarkResults {
        presets: preset_results,
        batch_sweep: sweep_results,
        parallel: parallel_result,
    }
}
