---
id: fixture_python_register_ocr_backend_trait_bridge
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

register_ocr_backend: trait bridge

```python title="Python"
from xberg import register_ocr_backend, unregister_ocr_backend, ExtractionConfig

def main() -> None:
    class _TestStub_register_ocr_backend_trait_bridge:
        def name(self):
            return "test-backend"
        def initialize(self):
            pass
        def shutdown(self):
            pass
        async def process_image(self, _p0, _p1):
            return {}
        def supports_language(self, _p0):
            return False
        def backend_type(self):
            return {}
    _ = register_ocr_backend(_TestStub_register_ocr_backend_trait_bridge())
    unregister_ocr_backend("test-backend")

main()

```
