---
id: fixture_node_config_extraction_timeout
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const config: ExtractionConfig = { extractionTimeoutSecs: 300 };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
