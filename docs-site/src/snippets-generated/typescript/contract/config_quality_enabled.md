---
id: fixture_node_config_quality_enabled
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const config: ExtractionConfig = { enableQualityProcessing: true };
  const result = await extract(input, config);
  console.log(result.results[0].qualityScore);
}

void main();

```
