---
id: fixture_node_api_extract_uri
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/pdf/fake_memo.pdf" };
  const result = await extract(input, undefined);
  console.log(result.results[0].content);
}

void main();

```
