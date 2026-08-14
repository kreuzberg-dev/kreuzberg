---
id: fixture_node_url_batch_mixed_inputs
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```typescript title="TypeScript"
import { ExtractionConfig, UrlExtractionMode, extractBatch } from "@xberg-io/xberg";
async function main() {
  const config: ExtractionConfig = { url: { mode: UrlExtractionMode.Document } };
  const result = await extractBatch([{ kind: "uri", uri: "https://example.com" }, { bytes: [66, 97, 116, 99, 104, 32, 98, 121, 116, 101, 115, 32, 99, 111, 110, 116, 101, 110, 116], filename: "inline.txt", kind: "bytes", mimeType: "text/plain" }], config);
  console.log(result);
}

void main();

```
