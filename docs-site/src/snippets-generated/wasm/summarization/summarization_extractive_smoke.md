---
id: fixture_wasm_summarization_extractive_smoke
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com/text/book_war_and_peace_1p.txt"; return _u0; })();
  const result = await extract(input, { summarization: { maxTokens: 80, strategy: "extractive" } });
  console.log(result.results[0].summary);
}

void main();

```
