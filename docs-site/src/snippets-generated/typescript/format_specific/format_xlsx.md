---
id: fixture_node_format_xlsx
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", uri: "https://example.com/xlsx/stanley_cups.xlsx" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
