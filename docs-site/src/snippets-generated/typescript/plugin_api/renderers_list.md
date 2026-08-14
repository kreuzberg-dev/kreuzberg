---
id: fixture_node_renderers_list
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List all registered renderers

```typescript title="TypeScript"
import { listRenderers } from "@xberg-io/xberg";
function main() {
  const result = listRenderers();
  console.log(result);
}

void main();

```
