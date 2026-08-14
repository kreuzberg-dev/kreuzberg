---
id: fixture_node_smoke_txt_basic
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "text/plain", uri: "https://example.com/text/report.txt" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
