---
id: fixture_node_config_embedding_plugin
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const config: ExtractionConfig = { chunking: { embedding: { maxEmbedDurationSecs: 30, model: { name: "test-plugin-backend", type: "plugin" }, normalize: true }, maxChars: 500, maxOverlap: 50 } };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
