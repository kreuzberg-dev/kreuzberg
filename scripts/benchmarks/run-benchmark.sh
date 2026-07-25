#!/usr/bin/env bash
set -euo pipefail

FRAMEWORK="${FRAMEWORK:-}"
MODE="${MODE:-}"
ITERATIONS="${ITERATIONS:-3}"
TIMEOUT="${TIMEOUT:-900}"
FIXTURES_DIR="${FIXTURES_DIR:-tools/benchmark-harness/fixtures}"
HARNESS_PATH="${HARNESS_PATH:-./target/release/benchmark-harness}"
MEASURE_QUALITY="${MEASURE_QUALITY:-false}"
OCR_ENABLED="${OCR_ENABLED:-false}"
OUTPUT_FORMAT="${OUTPUT_FORMAT:-markdown}"
COHORT="${COHORT:-}"
BATCH_SIZE="${BATCH_SIZE:-}"
SHARD="${SHARD:-}"
XBERG_MAX_THREADS="${XBERG_MAX_THREADS:-4}"

if [ -z "$FRAMEWORK" ] || [ -z "$MODE" ]; then
  echo "::error::FRAMEWORK and MODE environment variables are required" >&2
  exit 1
fi

if [ -n "$COHORT" ]; then
  if [ ! -f "$COHORT" ]; then
    echo "::error::Benchmark cohort does not exist: $COHORT" >&2
    exit 1
  fi
  if ! jq -e '
    type == "object" and
    .schema_version == 1 and
    (.name | type == "string" and length > 0) and
    (.batch_size | type == "number" and . > 0 and floor == .) and
    (.fixtures | type == "array" and length > 0) and
    ((.fixtures | length) % .batch_size == 0)
  ' "$COHORT" >/dev/null 2>&1; then
    echo "::error::Benchmark cohort has an invalid manifest shape: $COHORT" >&2
    exit 1
  fi
fi

if [ -n "$COHORT" ] && [ -n "$SHARD" ]; then
  echo "::error::COHORT and SHARD cannot be used together" >&2
  exit 1
fi

if [ "$MODE" = "batch" ] && [ -n "$COHORT" ] && [ -z "$BATCH_SIZE" ]; then
  echo "::error::BATCH_SIZE is required when MODE=batch uses a cohort" >&2
  exit 1
fi

if [ -n "$BATCH_SIZE" ]; then
  if [[ ! "$BATCH_SIZE" =~ ^[1-9][0-9]*$ ]]; then
    echo "::error::BATCH_SIZE must be a positive integer: $BATCH_SIZE" >&2
    exit 1
  fi
  if [ "$MODE" != "batch" ]; then
    echo "::error::BATCH_SIZE is only valid when MODE=batch" >&2
    exit 1
  fi
  if [ -n "$COHORT" ]; then
    COHORT_BATCH_SIZE="$(jq -r '.batch_size' "$COHORT")"
    if [ "$BATCH_SIZE" != "$COHORT_BATCH_SIZE" ]; then
      echo "::error::BATCH_SIZE $BATCH_SIZE does not match cohort batch_size $COHORT_BATCH_SIZE: $COHORT" >&2
      exit 1
    fi
  fi
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

source "${REPO_ROOT}/scripts/lib/common.sh"
source "${REPO_ROOT}/scripts/lib/library-paths.sh"

validate_repo_root "$REPO_ROOT" || exit 1

setup_go_paths "$REPO_ROOT"
setup_onnx_paths

OUTPUT_DIR="benchmark-results/${FRAMEWORK}-${OUTPUT_FORMAT}-${MODE}"
rm -rf "${OUTPUT_DIR}"

MAX_CONCURRENT=$([[ "$MODE" == "single-file" ]] && echo 1 || echo 4)

EXTRA_ARGS=()
if [ "$MEASURE_QUALITY" = "true" ]; then
  EXTRA_ARGS+=("--measure-quality")
fi
if [ "$OCR_ENABLED" = "true" ]; then
  EXTRA_ARGS+=("--ocr")
fi
if [ -n "$SHARD" ]; then
  EXTRA_ARGS+=("--shard" "${SHARD}")
fi
if [ -n "$COHORT" ]; then
  EXTRA_ARGS+=("--cohort" "${COHORT}")
fi
if [ -n "$BATCH_SIZE" ]; then
  EXTRA_ARGS+=("--batch-size" "${BATCH_SIZE}")
fi

BENCHMARK_DEBUG=1 "${HARNESS_PATH}" \
  run \
  --fixtures "${FIXTURES_DIR}" \
  --frameworks "${FRAMEWORK}" \
  --output "${OUTPUT_DIR}" \
  --iterations "${ITERATIONS}" \
  --timeout "${TIMEOUT}" \
  --mode "${MODE}" \
  --max-concurrent "${MAX_CONCURRENT}" \
  --xberg-max-threads "${XBERG_MAX_THREADS}" \
  --output-format "${OUTPUT_FORMAT}" \
  "${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}"
