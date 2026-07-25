#!/usr/bin/env bash
# Run the in-crate #[wasm_bindgen_test] suites for xberg-wasm under Node.
#
# The suites live in the crate's hand-written modules (src/engine.rs,
# src/bridge/ner_model.rs) because
# the generated manifest builds only a cdylib, which integration tests under
# tests/ cannot link against. They run under Node because wasm-bindgen's test
# glue carries the same unresolvable "env" / "wasi_snapshot_preview1" imports
# (from the WASI-cross-compiled Tesseract in ocr-wasm) that
# crates/xberg-wasm/scripts/fix-wasi-imports.mjs patches out of the published
# package. The test runner generates its glue on the fly with no patch hook,
# so NODE_PATH supplies stub modules for those imports instead; see
# crates/xberg-wasm/test-shims/. ~keep
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"

# CI exports RUSTFLAGS for the native builds (onnxruntime -L paths and
# -Wl,-rpath,$ORIGIN link args that rust-lld rejects). The env var overrides
# .cargo/config.toml's [target.wasm32-unknown-unknown] rustflags entirely,
# which also drops flags the wasm build needs (getrandom_backend, bulk-memory,
# --allow-multiple-definition). Unset it so the config-file flags govern. ~keep
unset RUSTFLAGS CARGO_ENCODED_RUSTFLAGS

if ! command -v wasm-pack >/dev/null 2>&1; then
  "$repo_root/scripts/ci/wasm/install-wasm-pack.sh"
fi

export NODE_PATH="$repo_root/crates/xberg-wasm/test-shims${NODE_PATH:+:$NODE_PATH}"

exec wasm-pack test --node "$repo_root/crates/xberg-wasm"
