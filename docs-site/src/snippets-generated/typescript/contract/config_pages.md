---
id: fixture_node_config_pages
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const config: ExtractionConfig = { pages: { extractPages: true, insertPageMarkers: true } };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
