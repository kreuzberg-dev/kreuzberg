---
id: fixture_python_list_reranker_backends
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```python title="Python"
from xberg import list_reranker_backends

def main() -> None:
    list_reranker_backends()
    print(result)

main()

```
