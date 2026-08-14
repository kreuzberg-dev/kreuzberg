---
id: fixture_python_renderers_clear
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Clear all renderers and verify list is empty

```python title="Python"
from xberg import clear_renderers

def main() -> None:
    _ = clear_renderers()

main()

```
