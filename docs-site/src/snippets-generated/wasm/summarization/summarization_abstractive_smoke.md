---
id: fixture_wasm_summarization_abstractive_smoke
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = ExtractInputKind.Uri; _u0.uri = "https://example.com/text/book_war_and_peace_1p.txt"; return _u0; })();
  const result = await extract(input, { summarization: { llm: { maxTokens: 200, model: "openai/gpt-4o-mini", temperature: 0.0 }, maxTokens: 150, strategy: "abstractive" } });
  console.log(result.results[0].summary);
}

void main();

```
