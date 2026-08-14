---
id: fixture_node_smoke_image_png
language: typescript
target: node
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```typescript title="TypeScript"
import { ExtractInput, ExtractInputKind, ExtractionConfig, extract } from "@xberg-io/xberg";
async function main() {
  const input: ExtractInput = { kind: ExtractInputKind.Uri, uri: "https://example.com/images/sample.png" };
  const config: ExtractionConfig = { disableOcr: true };
  const result = await extract(input, config);
  console.log(result);
}

void main();

```
