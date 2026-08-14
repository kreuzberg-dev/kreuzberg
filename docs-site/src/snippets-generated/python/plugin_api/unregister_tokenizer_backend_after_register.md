---
id: fixture_python_unregister_tokenizer_backend_after_register
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

unregister_tokenizer_backend

```python title="Python"
from xberg import unregister_tokenizer_backend, ExtractionConfig

def main() -> None:
    name = "test-tokenizer-backend"
    unregister_tokenizer_backend(name)

main()

```
