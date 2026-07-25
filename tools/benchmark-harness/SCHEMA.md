# Aggregation Schema v2.6.0

This document describes the structure of `aggregated.json` produced by `benchmark-harness consolidate`.

## Top-level Shape

```json
{
  "schema_version": "2.6.0",
  "by_framework_mode": {
    "<aggregate_key>": {
      /* FrameworkModeAggregation */
    }
  },
  "disk_sizes": {
    "framework": {
      /* DiskSizeInfo */
    }
  },
  "comparison": {
    /* ComparisonData */
  },
  "per_fixture_results": [
    /* PerFixtureRow[] */
  ],
  "metadata": {
    /* ConsolidationMetadata */
  }
}
```

## Output Format Discriminator

The `output_format` field determines:

- **`markdown`**: Supports all metrics including SF1 (structural F1), layout percentiles, and all ranking tables
- **`plaintext`**: Text-only extraction; SF1 and layout percentiles are `null`; plaintext frameworks never appear in SF1 rankings

## by_framework_mode

Key format differs by framework family:

- **xberg** (`xberg-*`): `{framework_name}:{mode}` — the output format is already encoded
  in the framework name (e.g. `xberg-markdown-baseline`), so repeating it in the key is
  redundant.
- **competitors** (all other frameworks): `{framework}:{output_format}:{mode}` — format is not
  encoded in the name, so the key carries it explicitly.

Examples:

- `xberg-markdown-baseline:single`
- `xberg-plaintext-paddle-ocr:batch`
- `unstructured:plaintext:single`
- `docling:markdown:single`

Each entry contains:

```json
{
  "framework": "string", // Framework name without mode suffix
  "output_format": "markdown|plaintext", // Output format used
  "mode": "single|batch|...", // Execution mode
  "cold_start": {
    /* DurationPercentiles */
  }, // Optional, if cold start data available
  "by_file_type": {
    "pdf": {
      "file_type": "pdf",
      "no_ocr": {
        /* PerformancePercentiles */
      },
      "with_ocr": {
        /* PerformancePercentiles */
      }
    }
  }
}
```

## PerformancePercentiles

Contains p50, p95, p99 for all metrics:

```json
{
  "successful_sample_count": 42,
  "total_sample_count": 50,
  "framework_errors": 0,
  "harness_errors": 5,
  "timeouts": 3,
  "empty_content": 0,
  "error_details": {
    "error message": 2
  },
  "duration": { "p50": 100.5, "p95": 150.2, "p99": 199.9 },
  "throughput": { "p50": 5.2, "p95": 4.8, "p99": 3.1 },
  "memory": { "p50": 150.0, "p95": 200.0, "p99": 250.0 },
  "cpu": { "p50": 50.0, "p95": 75.0, "p99": 90.0 }, // Optional
  "extraction_duration": { "p50": 80.0, "p95": 120.0, "p99": 160.0 }, // Optional
  "quality": {
    /* QualityPercentiles */
  }, // Optional, if quality data available
  "success_rate_percent": 84.0
}
```

## QualityPercentiles

Includes p50, p95, p99 for all F1 metrics. Layout percentiles are `null` for plaintext-only frameworks:

```json
{
  "f1_text_p50": 0.92,
  "f1_text_p95": 0.88,
  "f1_text_p99": 0.75,
  "f1_numeric_p50": 0.85,
  "f1_numeric_p95": 0.8,
  "f1_numeric_p99": 0.7,
  "f1_layout_p50": 0.78, // null for plaintext output format
  "f1_layout_p95": 0.72, // null for plaintext output format
  "f1_layout_p99": 0.65, // null for plaintext output format
  "quality_score_p50": 0.85,
  "quality_score_p95": 0.8,
  "quality_score_p99": 0.7
}
```

## PerFixtureRow

One row per unique combination of (framework, output_format, execution_mode, fixture_id, ocr):

```json
{
  "framework": "xberg-markdown-baseline",
  "output_format": "markdown",
  "execution_mode": "single",
  "ocr": false,
  "fixture_id": "sample_doc_1",
  "file_type": "pdf",
  "duration_ms": 125.4,
  "peak_memory_mb": 180.5,
  "f1_text": 0.92,
  "f1_layout": 0.78, // null for plaintext mode
  "f1_numeric": 0.85,
  "quality_score": 0.85,
  "correct": true,
  "success": true,
  "error_kind": null // "FrameworkError", "HarnessError", "Timeout", etc. if !success
}
```

`ocr` is `true` or `false` only when actual OCR usage is known. It is `null` when the framework did not report OCR usage.

## Migration from v2.5.0 to v2.6.0

- `PerFixtureRow.ocr` changed from a required boolean to a nullable boolean so unknown OCR usage is not mislabeled as `false`.

## ComparisonData

Contains all cross-framework rankings split by output format for quality metrics:

```json
{
  "performance_ranking": [
    /* RankedFramework[] */
  ],
  "throughput_ranking": [
    /* RankedFramework[] */
  ],
  "memory_ranking": [
    /* RankedFramework[] */
  ],
  "cpu_ranking": [
    /* RankedFramework[] */
  ],
  "quality_ranking_markdown": [
    /* RankedFramework[] — markdown-only (combined quality with SF1 term) */
  ],
  "quality_ranking_plaintext": [
    /* RankedFramework[] — plaintext-only (combined quality, no SF1 term) */
  ],
  "pdf_quality_ranking_markdown": [
    /* RankedFramework[] — markdown-only, never plaintext */
  ],
  "pdf_quality_ranking_plaintext": [
    /* RankedFramework[] — plaintext-only */
  ],
  "pdf_tf1_ranking_markdown": [
    /* RankedFramework[] — markdown-only */
  ],
  "pdf_tf1_ranking_plaintext": [
    /* RankedFramework[] — plaintext-only */
  ],
  "pdf_sf1_ranking_markdown": [
    /* RankedFramework[] — markdown-only, never plaintext */
  ],
  "deltas_vs_baseline": {
    "<aggregate_key>": {
      /* DeltaMetrics */
    }
  }
}
```

### RankedFramework

```json
{
  "framework_mode": "xberg-markdown-baseline:single",
  "rank": 1,
  "value": 95.5, // The metric value (duration, throughput, etc.)
  "relative": 1.0 // Ratio relative to best (1.0 = best)
}
```

## Migration from v2.3.0 to v2.4.0

### Breaking Changes

1. **Schema version**: Bumped to `"2.4.0"`
2. **Xberg aggregate key format**: Changed from `framework:output_format:mode` to
   `framework_name:mode` for all `xberg-*` frameworks. Competitor key format
   (`framework:output_format:mode`) is unchanged.

### Xberg Consolidation

Language-binding frameworks (`xberg-py`, `xberg-node`, `xberg-rb`, `xberg-go`,
`xberg-java`, `xberg-csharp`, `xberg-elixir`, `xberg-php`, `xberg-rust`, etc.)
have been removed. They are replaced by three native pipelines run directly via the xberg CLI:

| Pipeline  | Markdown name                   | Plaintext name                   |
| --------- | ------------------------------- | -------------------------------- |
| Baseline  | `xberg-markdown-baseline`   | `xberg-plaintext-baseline`   |
| Layout    | `xberg-markdown-layout`     | `xberg-plaintext-layout`     |
| PaddleOCR | `xberg-markdown-paddle-ocr` | `xberg-plaintext-paddle-ocr` |

Batch variants append `-batch` to the framework name (e.g. `xberg-markdown-baseline-batch`),
which the harness normalises to aggregate key `xberg-markdown-baseline:batch`.

### Run provenance sidecar

The `run` command writes `provenance.json` beside the backward-compatible `results.json` array.
Schema version 2 records the Xberg repository commit/dirty bit, the ordered fixture descriptors
and document BLAKE3 digests, cohort manifest identity, adapter versions and executable digests,
explicit model revision identities, timing configuration, fixed batch partitions, requested
workers, framework-specific worker semantics, and the configured Xberg thread budget.
Local absolute paths are never serialized.

For Xberg rows, `frameworks[].configured_thread_budget` records an explicit
`--xberg-max-threads` value in either mode. For native batch rows without an explicit
value, it records the legacy `--max-concurrent` fallback passed to `xberg batch
--max-threads`. It is distinct from
`frameworks[].requested_workers`, which records the `--max-concurrent` document-concurrency
cap. `effective_workers` remains `null` because Xberg resolves effective document concurrency
from the workload. Non-Xberg rows and automatic-budget single-file rows omit
`configured_thread_budget`.

#### Run provenance migration from schema 1 to schema 2

- Added optional `FrameworkProvenance.configured_thread_budget`.
- Existing schema-1 sidecars remain readable; a missing field deserializes as `null`.
- Consumers should treat `null` as unavailable, not infer a thread budget from worker fields.
- Native-batch producers that omit `--xberg-max-threads` record the legacy fallback
  value from `--max-concurrent`; single-file producers omit the field.

### Key Format Rationale

The format component is implicit in the xberg framework name itself. Duplicating it in the
aggregate key (`xberg-markdown-baseline:markdown:single`) would be redundant and confusing.
Competitor names carry no format information, so they continue to need it in the key
(`docling:markdown:single`).

## Migration from v2.2.0 to v2.3.0

### Breaking Changes

1. **Schema version**: Bumped to `"2.3.0"`
2. **Framework key format**: Changed from `framework:mode` to `framework:output_format:mode`
3. **QualityPercentiles**: Added p95 and p99 percentiles for all F1 metrics; `f1_layout_*` fields are now optional (null for plaintext)
4. **FrameworkModeAggregation**: Added `output_format` field
5. **ComparisonData**: Replaced `pdf_tf1_ranking` with `pdf_tf1_ranking_markdown` and `pdf_tf1_ranking_plaintext`; `pdf_sf1_ranking` renamed to `pdf_sf1_ranking_markdown` (now markdown-only)

### New Fields

- `per_fixture_results`: Array of individual fixture results preserving per-file measurements
- `PerFixtureRow`: New struct capturing individual extraction outcomes

### Plaintext-only Behavior

- Plaintext frameworks NEVER appear in `pdf_sf1_ranking_markdown`
- Plaintext frameworks NEVER appear in `pdf_tf1_ranking_markdown` (they get their own `pdf_tf1_ranking_plaintext`)
- SF1 and layout percentiles are `null` for plaintext output format
- All performance rankings (speed, memory, throughput) include both formats without discrimination

## ConsolidationMetadata

```json
{
  "total_results": 500,
  "framework_count": 5,
  "file_type_count": 8,
  "shared_corpus_markdown": ["pdf"],
  "shared_corpus_plaintext": ["pdf"],
  "timestamp": "2025-05-09T10:15:30Z"
}
```

- **`framework_count`** counts *logical* frameworks: all `xberg-*` pipeline variants collapse to a
  single `xberg` before counting (so 7 competitors + xberg = 8, not 11).
- **`shared_corpus_markdown` / `shared_corpus_plaintext`** are the file types the "overall"
  `quality_ranking_markdown` / `quality_ranking_plaintext` are actually computed over — the
  intersection of file types every candidate framework of that format attempted. When a
  single-format framework (e.g. PDF-only `liteparse`/`mineru`) is in the pool, this collapses to
  that one type (e.g. `["pdf"]`), and the "overall" ranking must be read as that-type-only rather
  than a true all-format comparison.
