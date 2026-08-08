#!/usr/bin/env bash
set -euo pipefail

workflow="${1:-.github/workflows/benchmarks.yaml}"
cache_action="${2:-.github/actions/cache-benchmark-harness/action.yml}"
workflow_content="$(<"$workflow")"
cache_action_content="$(<"$cache_action")"

extract_job() {
  local job_name="$1"
  awk -v header="  ${job_name}:" '
    $0 == header { in_job = 1; print; next }
    in_job && /^  [[:alnum:]_-]+:/ { exit }
    in_job { print }
  '
}

extract_named_step() {
  local step_name="$1"
  awk -v header="      - name: ${step_name}" '
    $0 == header { in_step = 1; print; next }
    in_step && /^      - / { exit }
    in_step { print }
  '
}

extract_with_inputs() {
  awk '
    $0 == "        with:" { in_with = 1; next }
    in_with && /^        [^ ]/ { exit }
    in_with { print }
  '
}

require_exact_input() {
  local inputs="$1"
  local key="$2"
  local value="$3"
  local description="$4"
  local key_count
  local expected_count

  key_count="$(grep -Ec "^[[:space:]]+${key}:" <<<"$inputs" || true)"
  expected_count="$(grep -Fxc "          ${key}: ${value}" <<<"$inputs" || true)"
  if [[ "$key_count" -ne 1 || "$expected_count" -ne 1 ]]; then
    echo "benchmark workflow validation failed: $description"
    exit 1
  fi
}

require_exact_cli_build() {
  local job="$1"
  local description="$2"
  local expected="cargo build --locked --release -p xberg-cli --features all,xberg/sceptre-ocr-tract"
  local commands
  local command_count

  commands="$(
    grep -E '^[[:space:]]+run:[[:space:]]+cargo build --locked --release -p xberg-cli' <<<"$job" |
      sed -E 's/^[[:space:]]*run:[[:space:]]*//' || true
  )"
  command_count="$(grep -c . <<<"$commands" || true)"

  if [[ "$command_count" -ne 1 || "$commands" != "$expected" ]]; then
    echo "benchmark workflow validation failed: $description"
    exit 1
  fi
}

require_no_cli_build() {
  local job="$1"
  local description="$2"
  local command_count

  command_count="$(
    grep -Ec '^[[:space:]]+run:[[:space:]]+cargo build --locked --release -p xberg-cli' <<<"$job" || true
  )"

  if [[ "$command_count" -ne 0 ]]; then
    echo "benchmark workflow validation failed: $description"
    exit 1
  fi
}

require_step() {
  local job="$1"
  local pattern="$2"
  local description="$3"

  if ! grep -qE "$pattern" <<<"$job"; then
    echo "benchmark workflow validation failed: $description"
    exit 1
  fi
}

setup_job="$(extract_job setup <<<"$workflow_content")"
aggregate_job="$(extract_job aggregate-and-publish <<<"$workflow_content")"
setup_rust_step="$(extract_named_step "Setup Rust" <<<"$setup_job")"
setup_rust_inputs="$(extract_with_inputs <<<"$setup_rust_step")"

require_exact_input "$setup_rust_inputs" use-sccache '"false"' \
  "setup Rust must disable per-object sccache uploads"
require_exact_input "$setup_rust_inputs" disable-cache '"false"' \
  "setup Rust must retain the coarse Cargo target cache"
require_exact_cli_build "$setup_job" \
  "setup must build exactly one all-feature CLI for benchmark size measurement"
# aggregate reuses the exact xberg-cli binary `setup` built and uploaded
# (benchmarks-target) rather than cold-rebuilding it. This is what guarantees
# installation-size consistency -- the measured binary is byte-identical to the
# benchmarked one -- and avoids a redundant release compile in the release job.
require_no_cli_build "$aggregate_job" \
  "aggregate must reuse the setup-built xberg-cli artifact, not cold-rebuild it"
require_step "$aggregate_job" '^[[:space:]]+- name: Download build artifacts \(harness binary \+ xberg-cli\)' \
  "aggregate must download the benchmarks-target artifact for size measurement"
require_step "$aggregate_job" 'restore-binary-permissions\.sh' \
  "aggregate must restore benchmark binary permissions after downloading the artifact"

app_token_step="$(extract_named_step "Create GitHub App token (if credentials available)" <<<"$aggregate_job")"
if grep -qE '^[[:space:]]+owner:' <<<"$app_token_step"; then
  echo "benchmark workflow validation failed: release token must default to the current repository"
  exit 1
fi

for cache_input in 'BUILD_ENV_HASH=' 'RUSTFLAGS:-' 'CARGO_BUILD_TARGET:-' 'rustc -vV' 'uname -s'; do
  if ! grep -Fq "$cache_input" <<<"$cache_action_content"; then
    echo "benchmark workflow validation failed: harness cache key omits build input $cache_input"
    exit 1
  fi
done

echo "benchmark workflow build configuration is valid"
