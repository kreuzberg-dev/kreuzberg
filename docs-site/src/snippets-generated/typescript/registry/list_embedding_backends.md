---
id: fixture_node_list_embedding_backends
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List embedding backends

```typescript title="TypeScript"
import { listEmbeddingBackends } from "@xberg-io/xberg";
function main() {
  const result = listEmbeddingBackends();
  console.log(result);
}

void main();

```
