---
id: fixture_python_embedding_backends_clear
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Clear all embedding backends and verify list is empty

```python title="Python"
from xberg import clear_embedding_backends

def main() -> None:
    clear_embedding_backends()

main()

```
