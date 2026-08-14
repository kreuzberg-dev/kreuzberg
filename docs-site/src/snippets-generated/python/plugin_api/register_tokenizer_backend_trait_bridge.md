---
id: fixture_python_register_tokenizer_backend_trait_bridge
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

register_tokenizer_backend: trait bridge

```python title="Python"
from xberg import register_tokenizer_backend, unregister_tokenizer_backend, ExtractionConfig

def main() -> None:
    class _TestStub_register_tokenizer_backend_trait_bridge:
        def name(self):
            return "test-tokenizer-backend"
        def initialize(self):
            pass
        def shutdown(self):
            pass
        def count_tokens(self, _p0):
            return 1
    _ = register_tokenizer_backend(_TestStub_register_tokenizer_backend_trait_bridge())
    unregister_tokenizer_backend("test-tokenizer-backend")

main()

```
