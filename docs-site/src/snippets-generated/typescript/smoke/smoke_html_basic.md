---
id: fixture_node_smoke_html_basic
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "text/html", uri: "https://example.com/html/simple_table.html" };
  const result = await extract(input, undefined);
  const [first] = result.results ?? [];
  for (const table of first?.tables) {
    console.log(table.rows);
  }
}

void main();

```
