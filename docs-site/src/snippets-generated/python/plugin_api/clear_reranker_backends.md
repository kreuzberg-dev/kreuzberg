---
id: fixture_python_clear_reranker_backends
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Clear all reranker backends and verify list is empty

```python title="Python"
from xberg import clear_reranker_backends

def main() -> None:
    clear_reranker_backends()

main()

```
