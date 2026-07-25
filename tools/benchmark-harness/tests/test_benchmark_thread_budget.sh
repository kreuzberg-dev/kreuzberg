#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
TEST_DIRECTORY="$(mktemp -d)"
trap 'rm -rf "$TEST_DIRECTORY"' EXIT

fail() {
  echo "test_benchmark_thread_budget: $*" >&2
  exit 1
}

FAKE_HARNESS="$TEST_DIRECTORY/benchmark-harness"
CAPTURE="$TEST_DIRECTORY/invocations.txt"
export CAPTURE
cat >"$FAKE_HARNESS" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "$*" >>"$CAPTURE"
SH
chmod +x "$FAKE_HARNESS"

FIXTURES="$TEST_DIRECTORY/fixtures"
mkdir -p "$FIXTURES"
printf '%s\n' '{"metadata":{"requires_ocr":false}}' >"$FIXTURES/heuristic.json"

run_ci_benchmark() {
  local mode="$1"
  local budget="${2:-}"
  local args=(
    FRAMEWORK=xberg-markdown-baseline
    MODE="$mode"
    FIXTURES_DIR="$FIXTURES"
    HARNESS_PATH="$FAKE_HARNESS"
  )
  if [ -n "$budget" ]; then
    args+=(XBERG_MAX_THREADS="$budget")
  fi
  (
    cd "$TEST_DIRECTORY"
    env "${args[@]}" "$REPO_ROOT/scripts/benchmarks/run-benchmark.sh"
  )
}

: >"$CAPTURE"
run_ci_benchmark single-file
run_ci_benchmark batch
grep -F -- "--mode single-file --max-concurrent 1 --xberg-max-threads 4" "$CAPTURE" >/dev/null ||
  fail "CI single-file run did not use the fixed default Xberg budget"
grep -F -- "--mode batch --max-concurrent 4 --xberg-max-threads 4" "$CAPTURE" >/dev/null ||
  fail "CI batch run did not use the fixed default Xberg budget"

: >"$CAPTURE"
run_ci_benchmark single-file 7
grep -F -- "--mode single-file --max-concurrent 1 --xberg-max-threads 7" "$CAPTURE" >/dev/null ||
  fail "CI single-file run did not forward the explicit Xberg budget"

FAKE_XBERG="$TEST_DIRECTORY/xberg"
printf '#!/usr/bin/env bash\n' >"$FAKE_XBERG"
chmod +x "$FAKE_XBERG"

run_local_benchmark() {
  local budget="${1:-}"
  local args=(
    HEURISTIC_FIXTURES="$FIXTURES"
    BATCH_HEURISTIC_FIXTURES="$FIXTURES"
    FRAMEWORKS=xberg-markdown-baseline
    BATCH_FRAMEWORKS=xberg-markdown-baseline-batch
    HARNESS="$FAKE_HARNESS"
    XBERG_CLI_BINARY="$FAKE_XBERG"
    OUT="$TEST_DIRECTORY/results"
    SKIP_BUILD=1
  )
  if [ -n "$budget" ]; then
    args+=(XBERG_MAX_THREADS="$budget")
  fi
  env "${args[@]}" bash "$REPO_ROOT/tools/benchmark-harness/scripts/bench_local.sh"
}

: >"$CAPTURE"
run_local_benchmark
grep -F -- "--mode single-file --max-concurrent 1 --xberg-max-threads 4" "$CAPTURE" >/dev/null ||
  fail "local single-file run did not use the fixed default Xberg budget"
grep -F -- "--mode batch --max-concurrent 4 --xberg-max-threads 4" "$CAPTURE" >/dev/null ||
  fail "local batch run did not use the fixed default Xberg budget"

: >"$CAPTURE"
run_local_benchmark 7
grep -F -- "--mode single-file --max-concurrent 1 --xberg-max-threads 7" "$CAPTURE" >/dev/null ||
  fail "local single-file run did not forward the explicit Xberg budget"
grep -F -- "--mode batch --max-concurrent 4 --xberg-max-threads 7" "$CAPTURE" >/dev/null ||
  fail "local batch run did not forward the explicit Xberg budget"

echo "test_benchmark_thread_budget: passed"
