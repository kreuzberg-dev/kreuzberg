---
id: fixture_python_post_processors_clear
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Clear all post-processors and verify list is empty

```python title="Python"
from xberg import clear_post_processors

def main() -> None:
    _ = clear_post_processors()

main()

```
