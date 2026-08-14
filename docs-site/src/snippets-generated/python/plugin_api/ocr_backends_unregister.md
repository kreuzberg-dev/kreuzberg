---
id: fixture_python_ocr_backends_unregister
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Unregister nonexistent OCR backend gracefully

```python title="Python"
from xberg import unregister_ocr_backend, ExtractionConfig

def main() -> None:
    name = "nonexistent-backend-xyz"
    unregister_ocr_backend(name)

main()

```
