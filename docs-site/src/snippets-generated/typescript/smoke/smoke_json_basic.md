---
id: fixture_node_smoke_json_basic
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "application/json", uri: "https://example.com/json/simple.json" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
