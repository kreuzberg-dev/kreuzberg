---
id: fixture_python_unregister_validator_after_register
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

unregister_validator

```python title="Python"
from xberg import unregister_validator, ExtractionConfig

def main() -> None:
    name = "test-validator"
    _ = unregister_validator(name)

main()

```
