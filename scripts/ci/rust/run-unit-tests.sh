#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/../../.." && pwd)}"

source "$REPO_ROOT/scripts/lib/common.sh"
source "$REPO_ROOT/scripts/lib/tessdata.sh"

validate_repo_root "$REPO_ROOT" || exit 1

cd "$REPO_ROOT"

echo "=== Running Rust unit tests ==="

setup_tessdata

echo "Test environment configuration:"
echo "  TESSDATA_PREFIX: ${TESSDATA_PREFIX:-not set}"
echo "  RUST_BACKTRACE: ${RUST_BACKTRACE:-not set}"
echo "  CARGO_TERM_COLOR: ${CARGO_TERM_COLOR:-not set}"

echo "Workspace information:"
echo "  Repository: $REPO_ROOT"
echo "  Excluded packages: xberg-e2e-generator, xberg-py, xberg-node, xberg-candle-ocr, xberg-gliner, xberg-cli, xberg-wasm, benchmark-harness"

if [ ! -d "$TESSDATA_PREFIX" ]; then
  echo "WARNING: TESSDATA_PREFIX directory not found: $TESSDATA_PREFIX"
  echo "Attempting to create it..."
  mkdir -p "$TESSDATA_PREFIX"
  ensure_tessdata "$TESSDATA_PREFIX"
fi

echo "Verifying Tesseract data files..."
for lang in eng osd; do
  langfile="$TESSDATA_PREFIX/${lang}.traineddata"
  if [ -f "$langfile" ]; then
    size=$(stat -f%z "$langfile" 2>/dev/null || stat -c%s "$langfile" 2>/dev/null || echo "unknown")
    echo "  ✓ ${lang}.traineddata (${size} bytes)"
  else
    echo "  WARNING: Missing ${lang}.traineddata"
  fi
done

if [ -n "${XBERG_PDFIUM_PREBUILT:-}" ]; then
  export LD_LIBRARY_PATH="${XBERG_PDFIUM_PREBUILT}/lib:${LD_LIBRARY_PATH:-}"
  export DYLD_LIBRARY_PATH="${XBERG_PDFIUM_PREBUILT}/lib:${DYLD_LIBRARY_PATH:-}"
  export DYLD_FALLBACK_LIBRARY_PATH="${XBERG_PDFIUM_PREBUILT}/lib:${DYLD_FALLBACK_LIBRARY_PATH:-}"
  echo "Library path configuration:"
  echo "  LD_LIBRARY_PATH: $LD_LIBRARY_PATH"
  echo "  DYLD_LIBRARY_PATH: $DYLD_LIBRARY_PATH"
  echo "  DYLD_FALLBACK_LIBRARY_PATH: $DYLD_FALLBACK_LIBRARY_PATH"
fi

echo "=== Starting cargo test ==="

# NOTE: We intentionally avoid `--all-features` for the `xberg` crate because
TEST_LOG="/tmp/cargo-test-$$.log"

if ! {
  # ~keep `--all-targets` runs --lib --bins --tests --examples --benches but excludes
  # ~keep `--doc`. The xberg crate still has rustdoc examples for private/internal
  # ~keep APIs; `cargo test -p xberg --features full --doc` currently fails those
  # ~keep examples because rustdoc compiles them as an external crate.
  echo "=== cargo test -p xberg --features full ==="
  RUST_BACKTRACE=full cargo test --locked -p xberg --features full --all-targets --verbose

  echo "=== cargo test --workspace (all features, excluding xberg) ==="
  extra_excludes=()
  extra_excludes+=(--exclude xberg-candle-ocr)
  # xberg-gliner: its cuda/metal features cannot build on CI runners, so
  # --all-features is unusable; tested separately below with an explicit
  # feature list. ~keep
  extra_excludes+=(--exclude xberg-gliner)
  extra_excludes+=(--exclude xberg-cli)
  extra_excludes+=(--exclude benchmark-harness)
  # xberg-wasm: a cdylib whose tests are all cfg(target_arch = "wasm32"), so a native
  # run covers nothing; they run under Node in the ci-e2e wasm leg. Excluding it also
  # keeps candle out of this build: its xberg dependency is not target-gated, so
  # wasm-target's ner-candle-wasm would pull gemm-f16 in on aarch64 (no fullfp16),
  # past the --exclude xberg-gliner guard above. Matches every Taskfile path. ~keep
  extra_excludes+=(--exclude xberg-wasm)
  RUST_BACKTRACE=full cargo test --locked \
    --workspace \
    --exclude xberg \
    --exclude xberg-e2e-generator \
    --exclude xberg-py \
    --exclude xberg-node \
    ${extra_excludes[@]+"${extra_excludes[@]}"} \
    --all-features \
    --all-targets \
    --verbose

  echo "=== cargo test -p xberg-gliner (explicit features) ==="
  # cuda/metal cannot build on CPU-only runners, so xberg-gliner gets an
  # explicit feature list instead of --all-features: the default ONNX
  # features everywhere, plus candle where it can build. Only Linux aarch64
  # drops candle: gemm-f16 (candle's matmul backend) carries aarch64 inline
  # asm that requires the fullfp16 target feature, which that runner's
  # baseline lacks ("instruction requires: fullfp16"). Apple Silicon
  # includes fullfp16 and runs the candle tests. ~keep
  gliner_features=(--features candle,ort-dynamic)
  if [ "$(uname -s)" = "Linux" ] && [ "$(uname -m)" = "aarch64" ]; then
    echo "Dropping the candle feature on Linux aarch64 (gemm-f16 needs fullfp16)"
    gliner_features=(--features ort-dynamic)
  fi
  RUST_BACKTRACE=full cargo test --locked -p xberg-gliner \
    ${gliner_features[@]+"${gliner_features[@]}"} \
    --all-targets --verbose
} 2>&1 | tee "$TEST_LOG"; then
  echo "=== Test execution failed ==="
  echo "Last 50 lines of test output:"
  tail -n 50 "$TEST_LOG"
  echo ""
  echo "Collecting diagnostic information..."
  echo "Disk space:"
  df -h . || du -h . 2>/dev/null | head -1
  echo "Cargo environment:"
  cargo --version
  rustc --version
  rm -f "$TEST_LOG"
  exit 1
fi

rm -f "$TEST_LOG"

echo "=== Tests complete ==="
