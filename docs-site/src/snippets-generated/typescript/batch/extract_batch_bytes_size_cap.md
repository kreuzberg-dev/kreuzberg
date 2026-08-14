---
id: fixture_node_extract_batch_bytes_size_cap
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```typescript title="TypeScript"
import { ExtractionConfig, XbergError, extractBatch } from "@xberg-io/xberg";
async function main() {
  const config: ExtractionConfig = { securityLimits: { maxContentSize: 1 } };
  try {
    await extractBatch([{ bytes: "test_documents/text/fake_text.txt", kind: "bytes", mimeType: "text/plain" }], config);
  } catch (error) {
    if (error instanceof XbergError) {
      console.error(`${error.name}: ${error.message}`);
    }
  }
}

void main();

```
