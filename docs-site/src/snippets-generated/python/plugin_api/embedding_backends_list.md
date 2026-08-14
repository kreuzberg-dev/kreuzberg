---
id: fixture_python_embedding_backends_list
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

List all registered embedding backends

```python title="Python"
from xberg import list_embedding_backends

def main() -> None:
    list_embedding_backends()
    print(result)

main()

```
