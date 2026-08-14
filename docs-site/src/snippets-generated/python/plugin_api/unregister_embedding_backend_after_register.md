---
id: fixture_python_unregister_embedding_backend_after_register
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

unregister_embedding_backend

```python title="Python"
from xberg import unregister_embedding_backend, ExtractionConfig

def main() -> None:
    name = "test-embedding-backend"
    _ = unregister_embedding_backend(name)

main()

```
