---
id: fixture_wasm_config_llm_structured_extraction
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, { structuredExtraction: { llm: { model: "openai/gpt-4o" }, schema: { properties: { date: { type: "string" }, summary: { type: "string" }, title: { type: "string" } }, required: ["title"], type: "object" }, schemaName: "memo_data" } });
  console.log(result.results[0].structuredData);
}

void main();

```
