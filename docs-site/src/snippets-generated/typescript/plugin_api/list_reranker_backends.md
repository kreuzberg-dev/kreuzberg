---
id: fixture_node_list_reranker_backends
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```typescript title="TypeScript"
import { listRerankerBackends } from "@xberg-io/xberg";
function main() {
  const result = listRerankerBackends();
  console.log(result);
}

void main();

```
