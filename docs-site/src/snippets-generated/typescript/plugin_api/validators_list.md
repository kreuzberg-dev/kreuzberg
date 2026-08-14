---
id: fixture_node_validators_list
language: typescript
target: node
level: typecheck
requires: []
side_effect: safe
---

List all registered validators

```typescript title="TypeScript"
import { listValidators } from "@xberg-io/xberg";
function main() {
  const result = listValidators();
  console.log(result);
}

void main();

```
