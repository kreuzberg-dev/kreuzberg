---
id: fixture_node_api_extract_batch_bytes_with_config
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ bytes: "test_documents/pdf/fake_memo.pdf", config: { outputFormat: "markdown" }, filename: "fake_memo.pdf", kind: "bytes" }], undefined);
  console.log(result);
}

void main();

```
