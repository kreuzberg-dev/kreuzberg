---
id: fixture_wasm_config_tree_sitter
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com/code/hello.py"; return _u0; })();
  const result = await extract(input, { treeSitter: { groups: ["web"], languages: ["python", "rust"], process: { comments: false, diagnostics: false, docstrings: false, exports: true, imports: true, structure: true, symbols: false } } });
  console.log(result);
}

void main();

```
