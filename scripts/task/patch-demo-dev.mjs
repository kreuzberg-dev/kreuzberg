#!/usr/bin/env node

import { readFileSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const src = join(root, "docs-site", "public", "demo.html");
const dest = join(root, "docs-site", "public", "demo-dev.html");
const ASSET_PORT = process.env.ASSET_PORT ?? "9000";

// Rewrite the production WASM CDN origin to the local asset server. The demo
// then takes its localhost branch: it loads pkg/web from this origin and skips
// the jsdelivr registry version lookup. This must match the exact literal in
// demo.html: `WASM_CDN_ORIGIN =
// "https://cdn.jsdelivr.net/npm/@xberg-io/xberg-wasm"`. The env/wasi ESM shim
// is baked into pkg/web at build time by
// crates/xberg-wasm/scripts/fix-wasi-imports.mjs (run by the demo:dev:build
// task), so the wasm glue needs no patching here. ~keep
const cdnRe = /https:\/\/cdn\.jsdelivr\.net\/npm\/@xberg-io\/xberg-wasm/g;

const patched = readFileSync(src, "utf8")
  .replace(cdnRe, `http://localhost:${ASSET_PORT}`)
  .replace(/<title>(.*?)<\/title>/, "<title>$1 [local dev]</title>")
  .replace(
    "</body>",
    `  <div style="position:fixed;bottom:12px;right:12px;background:#1a172a;border:1px solid #58FBDA55;color:#58FBDA;font-family:monospace;font-size:11px;padding:6px 10px;border-radius:6px;z-index:9999">
    local dev · assets: localhost:${ASSET_PORT}
  </div>\n</body>`,
  );

writeFileSync(dest, patched, "utf8");
console.log(`patch-demo-dev: docs-site/public/demo-dev.html → http://localhost:8001/demo-dev.html`);
console.log(`  assets served from http://localhost:${ASSET_PORT}`);
