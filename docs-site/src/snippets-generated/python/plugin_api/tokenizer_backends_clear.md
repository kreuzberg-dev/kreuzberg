---
id: fixture_python_tokenizer_backends_clear
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Clear all tokenizer backends and verify list is empty

```python title="Python"
from xberg import clear_tokenizer_backends

def main() -> None:
    _ = clear_tokenizer_backends()

main()

```
