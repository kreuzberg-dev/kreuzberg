---
id: fixture_node_embedding_backends_list
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List all registered embedding backends

```typescript title="TypeScript"
import { listEmbeddingBackends } from "@xberg-io/xberg";
function main() {
  const result = listEmbeddingBackends();
  console.log(result);
}

void main();

```
