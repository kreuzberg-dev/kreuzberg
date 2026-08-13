#!/usr/bin/env node
/**
 * Post-build patch for the wasm-pack target output (nodejs + web + bundler +
 * deno).
 *
 * Problem: the `ocr-wasm` feature links `xberg-tesseract` (Tesseract +
 * Leptonica cross-compiled with the WASI SDK for wasm32-wasi) into the
 * wasm32-unknown-unknown module produced for this crate (see
 * `.cargo/config.toml`'s `--allow-multiple-definition` for the target). The
 * resulting `xberg_wasm_bg.wasm` therefore imports the `env` (Leptonica's
 * `mkstemp`/`system`) and `wasi_snapshot_preview1` (standard WASI preview1
 * syscalls) modules. wasm-bindgen's glue re-exposes those as host imports:
 *
 *   - nodejs target (CommonJS):
 *       const import1 = require("env");
 *       const import3 = require("wasi_snapshot_preview1");
 *   - web / bundler / deno targets (ESM):
 *       import * as import1 from "env";
 *       import * as import3 from "wasi_snapshot_preview1";
 *
 * Neither "env" nor "wasi_snapshot_preview1" is a real module. On Node this
 * throws `Cannot find module 'env'` on require; in a **browser** the ESM
 * loader throws `Failed to resolve module specifier "env"` before the WASM is
 * ever instantiated. Both failure modes must be fixed, so every generated
 * target — not just nodejs — needs the imports stripped and replaced with
 * inline stubs. (An earlier version of this script patched only the nodejs
 * target on the incorrect assumption that ESM targets were unaffected; the
 * browser ESM loader proves otherwise, which broke the live WASM demo.)
 *
 * Fix: strip the import statements (require for CJS, `import * as` for ESM),
 * replace every reference to the per-symbol import variables with two shared
 * stub objects, and (CJS only) deduplicate the resulting object-literal keys.
 * Duplicate object keys are harmless once every occurrence points at the same
 * stub object (JS keeps the last value per key), so the ESM path skips the
 * dedup. All OCR/table-detection work happens on image bytes already resident
 * in WASM linear memory, so the stubs never need a real filesystem or shell:
 * they report "no filesystem here" (WASI errno values) for path-based
 * syscalls and answer `fd_read`/`fd_write`/`clock_time_get`/`environ_*` with
 * harmless real values so Tesseract/Leptonica's initialization and buffered
 * stdio calls succeed.
 *
 * Idempotent: running twice is a no-op (guarded on the `__wasi_stubs__`
 * marker). The pkg directory defaults to `../pkg` relative to this script but
 * can be overridden with `XBERG_WASM_PKG_DIR` for testing against an extracted
 * published bundle.
 */
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const pkgDir = process.env.XBERG_WASM_PKG_DIR ?? path.join(__dirname, "..", "pkg");

// Injected verbatim just before `function __wbg_get_imports()` in every target.
// ~keep
const STUB_CODE = `// __wasi_stubs__ - inline replacements for the unresolvable "env" /
// "wasi_snapshot_preview1" import targets. See fix-wasi-imports.mjs for
// the full rationale; this block is injected by that script.
let __wasi_mem_ref = { memory: null };
function __wasi_view() {
    if (!__wasi_mem_ref.memory) return null;
    return new DataView(__wasi_mem_ref.memory.buffer);
}

// Leptonica's system()/mkstemp() shell-exec and temp-file helpers are never
// reached on the in-memory OCR path. The Proxy catch-all covers any other
// unresolved libc/env symbol the WASI-SDK link left dangling.
const __env_stubs__ = new Proxy({
    system: () => -1,
    mkstemp: () => -1,
}, {
    get(target, prop) {
        if (prop in target) return target[prop];
        return () => {};
    }
});

// WASI preview1 stubs. Functions with output pointers write real values into
// WASM memory; everything filesystem-shaped reports absence (EBADF/ENOSYS)
// since this embedding never preopens a directory.
const __wasi_stubs__ = {
    fd_close: () => 0,
    fd_read: (fd, iovs_ptr, iovs_len, nread_ptr) => {
        const v = __wasi_view();
        if (v && nread_ptr) v.setUint32(nread_ptr, 0, true);
        return 0;
    },
    fd_write: (fd, iovs_ptr, iovs_len, nwritten_ptr) => {
        const v = __wasi_view();
        if (v) {
            let total = 0;
            for (let i = 0; i < iovs_len; i++) {
                total += v.getUint32(iovs_ptr + i * 8 + 4, true);
            }
            if (nwritten_ptr) v.setUint32(nwritten_ptr, total, true);
        }
        return 0;
    },
    fd_seek: (fd, offset_lo, offset_hi, whence, newoffset_ptr) => {
        const v = __wasi_view();
        if (v && newoffset_ptr) {
            v.setUint32(newoffset_ptr, 0, true);
            v.setUint32(newoffset_ptr + 4, 0, true);
        }
        return 0;
    },
    fd_fdstat_get: (fd, fdstat_ptr) => {
        const v = __wasi_view();
        if (v && fdstat_ptr) {
            v.setUint8(fdstat_ptr, fd <= 2 ? 2 : 4);
            v.setUint16(fdstat_ptr + 2, 0, true);
            v.setBigUint64(fdstat_ptr + 8, 0xffffffffffffffffn, true);
            v.setBigUint64(fdstat_ptr + 16, 0xffffffffffffffffn, true);
        }
        return 0;
    },
    fd_fdstat_set_flags: (fd, flags) => 0,
    fd_prestat_get: (fd, prestat_ptr) => 8, // EBADF - no preopened dirs
    fd_prestat_dir_name: (fd, path_ptr, path_len) => 8, // EBADF
    environ_get: (environ_ptr, environ_buf_ptr) => 0,
    environ_sizes_get: (count_ptr, buf_size_ptr) => {
        const v = __wasi_view();
        if (v) {
            if (count_ptr) v.setUint32(count_ptr, 0, true);
            if (buf_size_ptr) v.setUint32(buf_size_ptr, 0, true);
        }
        return 0;
    },
    clock_time_get: (clock_id, precision, time_ptr) => {
        const v = __wasi_view();
        if (v && time_ptr) {
            v.setBigUint64(time_ptr, BigInt(Math.floor(Date.now() * 1e6)), true);
        }
        return 0;
    },
    path_create_directory: (fd, path_ptr, path_len) => 63, // ENOSYS
    path_filestat_get: (fd, flags, path_ptr, path_len, filestat_ptr) => 63,
    path_open: (dirfd, dirflags, path_ptr, path_len, oflags, fs_rights_base_lo, fs_rights_base_hi, fs_rights_inheriting_lo, fs_rights_inheriting_hi, fdflags, fd_ptr) => 63,
    path_remove_directory: (fd, path_ptr, path_len) => 63,
    path_unlink_file: (fd, path_ptr, path_len) => 63,
    proc_exit: (code) => { throw new Error(\`WASM proc_exit called with code \${code}\`); },
    sched_yield: () => 0,
};

`;

function injectStubs(content) {
  const getImportsIdx = content.indexOf("function __wbg_get_imports()");
  if (getImportsIdx === -1) {
    throw new Error("could not find __wbg_get_imports()");
  }
  return content.slice(0, getImportsIdx) + STUB_CODE + content.slice(getImportsIdx);
}

function patchCjs(jsFile) {
  let content = fs.readFileSync(jsFile, "utf-8");

  const hasCjsImports = content.includes('require("env")') || content.includes('require("wasi_snapshot_preview1")');
  if (!hasCjsImports) {
    console.log(`[fix-wasi-imports] ${rel(jsFile)}: no env/wasi require() calls, skipping.`);
    return;
  }

  console.log(`[fix-wasi-imports] ${rel(jsFile)}: patching require("env") / require("wasi_snapshot_preview1")…`);

  const cjsPattern = /^const (import\d+) = require\("(env|wasi_snapshot_preview1)"\);?$/gm;
  const envImports = [];
  const wasiImports = [];
  for (const match of content.matchAll(cjsPattern)) {
    const [, varName, moduleName] = match;
    (moduleName === "env" ? envImports : wasiImports).push(varName);
  }

  content = content.replace(/^const import\d+ = require\("(env|wasi_snapshot_preview1)"\);?\n/gm, "");
  content = injectStubs(content);

  // Precise reference replacement on the import-object return block so we never
  // corrupt identifiers by substring (import1 ⊂ import10). Every reference of
  // shape `"env": importN` / `"wasi_snapshot_preview1": importN` is rewritten.
  // ~keep
  content = content
    .replace(/("env":\s*)import\d+/g, "$1__env_stubs__")
    .replace(/("wasi_snapshot_preview1":\s*)import\d+/g, "$1__wasi_stubs__");

  // Deduplicate the "env" / "wasi_snapshot_preview1" keys in the import object
  // literal — cosmetic (all point at the same stub) but keeps the glue tidy.
  // ~keep
  const returnBlockStart = content.indexOf('"./xberg_wasm_bg.js": import0,');
  if (returnBlockStart !== -1) {
    const returnBlockEnd = content.indexOf("};", returnBlockStart);
    if (returnBlockEnd !== -1) {
      const returnBlock = content.slice(returnBlockStart, returnBlockEnd);
      let seenEnv = false;
      let seenWasi = false;
      const dedupedLines = returnBlock.split("\n").filter((line) => {
        const trimmed = line.trim();
        if (trimmed.startsWith('"env"')) {
          if (seenEnv) return false;
          seenEnv = true;
        }
        if (trimmed.startsWith('"wasi_snapshot_preview1"')) {
          if (seenWasi) return false;
          seenWasi = true;
        }
        return true;
      });
      content = content.slice(0, returnBlockStart) + dedupedLines.join("\n") + content.slice(returnBlockEnd);
    }
  }

  const instantiatePattern = /^(let wasmInstance = new WebAssembly\.Instance\(.*\);)$/m;
  if (instantiatePattern.test(content)) {
    content = content.replace(
      instantiatePattern,
      "$1\n// Populate WASI memory reference for stubs that write output values\n__wasi_mem_ref.memory = wasmInstance.exports.memory;",
    );
  } else {
    console.log(`[fix-wasi-imports] ${rel(jsFile)}: WARNING: could not find WebAssembly.Instance to wire memory.`);
  }

  fs.writeFileSync(jsFile, content);
  console.log(
    `[fix-wasi-imports] ${rel(jsFile)}: replaced ${
      envImports.length + wasiImports.length
    } require() import(s) with stubs.`,
  );
}

function patchEsm(jsFile) {
  let content = fs.readFileSync(jsFile, "utf-8");

  const esmPattern = /^import \* as (import\d+) from "(env|wasi_snapshot_preview1)"\s*;?$/gm;
  const matches = [...content.matchAll(esmPattern)];
  if (matches.length === 0) {
    console.log(`[fix-wasi-imports] ${rel(jsFile)}: no env/wasi ESM imports, skipping.`);
    return;
  }

  console.log(`[fix-wasi-imports] ${rel(jsFile)}: patching import * from "env"/"wasi_snapshot_preview1"…`);

  content = content.replace(/^import \* as import\d+ from "(env|wasi_snapshot_preview1)"\s*;?\n/gm, "");
  content = injectStubs(content);

  // Precise reference replacement (avoids import1 ⊂ import10 corruption). ~keep
  content = content
    .replace(/("env":\s*)import\d+/g, "$1__env_stubs__")
    .replace(/("wasi_snapshot_preview1":\s*)import\d+/g, "$1__wasi_stubs__");

  // Wire linear memory so stubs that write output values can reach it. Two
  // shapes exist: web/bundler use __wbg_finalize_init(instance, module); deno
  // instantiates at top level via instantiateStreaming. ~keep
  const finalizePattern =
    /(function __wbg_finalize_init\(instance, module\) \{\s*\n\s*wasmInstance = instance;\s*\n\s*wasm = instance\.exports;)/;
  const denoPattern = /(\n\s*const wasm = wasmInstance\.exports;)/;
  if (finalizePattern.test(content)) {
    content = content.replace(
      finalizePattern,
      "$1\n    // Populate WASI memory reference for stubs that write output values\n    __wasi_mem_ref.memory = instance.exports.memory;",
    );
  } else if (denoPattern.test(content)) {
    content = content.replace(
      denoPattern,
      "$1\n// Populate WASI memory reference for stubs that write output values\n__wasi_mem_ref.memory = wasmInstance.exports.memory;",
    );
  } else {
    console.log(`[fix-wasi-imports] ${rel(jsFile)}: WARNING: could not find an init site to wire memory.`);
  }

  fs.writeFileSync(jsFile, content);
  console.log(`[fix-wasi-imports] ${rel(jsFile)}: replaced ${matches.length} ESM import(s) with stubs.`);
}

function rel(p) {
  return path.relative(pkgDir, p) || path.basename(p);
}

function patchTarget(target, kind) {
  const jsFile = path.join(pkgDir, target, "xberg_wasm.js");
  if (!fs.existsSync(jsFile)) {
    console.log(`[fix-wasi-imports] ${target}/xberg_wasm.js not found, skipping.`);
    return;
  }
  const content = fs.readFileSync(jsFile, "utf-8");
  if (content.includes("__wasi_stubs__")) {
    console.log(`[fix-wasi-imports] ${target}/xberg_wasm.js already patched, skipping.`);
    return;
  }
  if (kind === "cjs") patchCjs(jsFile);
  else patchEsm(jsFile);
}

patchTarget("nodejs", "cjs");
patchTarget("web", "esm");
patchTarget("bundler", "esm");
patchTarget("deno", "esm");

console.log("[fix-wasi-imports] done.");
