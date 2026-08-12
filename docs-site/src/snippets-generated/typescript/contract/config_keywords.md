---
id: fixture_node_config_keywords
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, KeywordAlgorithm, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const config: ExtractionConfig = { keywords: { algorithm: KeywordAlgorithm.Yake, maxKeywords: 10 } };
  const result = await extract(input, config);
  const [first] = result.results ?? [];
  for (const keyword of first?.keywords) {
    console.log(keyword.text);
    console.log(keyword.score);
  }
}

void main();

```
