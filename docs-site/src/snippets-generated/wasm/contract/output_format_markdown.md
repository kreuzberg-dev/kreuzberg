---
id: fixture_wasm_output_format_markdown
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, { outputFormat: "markdown" });
  console.log(result);
}

void main();

```
