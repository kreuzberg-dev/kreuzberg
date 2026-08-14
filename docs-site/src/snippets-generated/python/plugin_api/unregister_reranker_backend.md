---
id: fixture_python_unregister_reranker_backend
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

unregister_reranker_backend

```python title="Python"
from xberg import unregister_reranker_backend, ExtractionConfig

def main() -> None:
    name = "test-reranker-backend"
    unregister_reranker_backend(name)

main()

```
