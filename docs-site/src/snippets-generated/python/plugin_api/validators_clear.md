---
id: fixture_python_validators_clear
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Clear all validators and verify list is empty

```python title="Python"
from xberg import clear_validators

def main() -> None:
    clear_validators()

main()

```
