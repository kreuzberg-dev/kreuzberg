// Stub for the unresolvable "env" import module that wasm-bindgen's Node
// test glue requires. The WASI-cross-compiled Tesseract/Leptonica stack in
// `ocr-wasm` leaves `system`/`mkstemp` (and potentially other libc symbols)
// dangling as `env` imports; the in-crate engine tests never reach that C
// code, so the stubs only need to exist for the module to instantiate.
//
// This mirrors `scripts/fix-wasi-imports.mjs`, which solves the same problem
// for the published package by patching the wasm-pack glue post-build. The
// test runner generates its glue on the fly with no patch hook, so these
// modules are supplied at require time instead, via NODE_PATH (see
// `scripts/ci/wasm/run-crate-tests.sh`). ~keep
module.exports = new Proxy(
  {
    system: () => -1,
    mkstemp: () => -1,
  },
  {
    get(target, prop) {
      if (prop in target) return target[prop];
      return () => {};
    },
  },
);
