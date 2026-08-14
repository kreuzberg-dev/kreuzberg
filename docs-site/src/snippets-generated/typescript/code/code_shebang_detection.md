---
id: fixture_node_code_shebang_detection
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, mimeType: "text/x-source-code", uri: "https://example.com/code/script.sh" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
