---
id: fixture_node_ocr_backends_list
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List all registered OCR backends

```typescript title="TypeScript"
import { listOcrBackends } from "@xberg-io/xberg";
function main() {
  const result = listOcrBackends();
  console.log(result);
}

void main();

```
