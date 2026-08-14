---
id: fixture_wasm_config_embedding_plugin
language: typescript
target: wasm
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```typescript title="WebAssembly"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg-wasm";
async function main() {
  const input: WasmExtractInput = (() => { const _u0 = WasmExtractInput.default(); _u0.kind = WasmExtractInputKind.Uri; _u0.uri = "https://example.com/pdf/fake_memo.pdf"; return _u0; })();
  const result = await extract(input, { chunking: { embedding: { maxEmbedDurationSecs: 30, model: { name: "test-plugin-backend", type: "plugin" }, normalize: true }, maxChars: 500, maxOverlap: 50 } });
  console.log(result);
}

void main();

```
