---
id: fixture_node_format_hwpx_standalone
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { filename: "simple.hwpx", kind: ExtractInputKind.Uri, mimeType: "application/haansofthwpx", uri: "https://example.com/hwpx/simple.hwpx" };
  const result = await extract(input, undefined);
  console.log(result);
}

void main();

```
