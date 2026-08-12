---
id: fixture_node_api_extract_batch_uri_with_config
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```typescript title="TypeScript"
import { extractBatch } from "@xberg-io/xberg";
async function main() {
  const result = await extractBatch([{ config: { outputFormat: "markdown" }, kind: "uri", uri: "https://example.com/pdf/fake_memo.pdf" }], undefined);
  for (const result of result.results) {
    console.log(result.content);
  }
}

void main();

```
