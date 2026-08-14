---
id: fixture_python_ocr_backends_clear
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Clear all OCR backends and verify list is empty

```python title="Python"
from xberg import clear_ocr_backends

def main() -> None:
    _ = clear_ocr_backends()

main()

```
