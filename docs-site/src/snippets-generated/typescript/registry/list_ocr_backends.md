---
id: fixture_node_list_ocr_backends
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List OCR backends

```typescript title="TypeScript"
import { listOcrBackends } from "@xberg-io/xberg";
function main() {
  const result = listOcrBackends();
  console.log(result);
}

void main();

```
