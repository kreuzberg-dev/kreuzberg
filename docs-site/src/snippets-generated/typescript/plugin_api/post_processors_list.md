---
id: fixture_node_post_processors_list
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List all registered post-processors

```typescript title="TypeScript"
import { listPostProcessors } from "@xberg-io/xberg";
function main() {
  const result = listPostProcessors();
  console.log(result);
}

void main();

```
