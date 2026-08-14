---
id: fixture_python_register_embedding_backend_trait_bridge
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

register_embedding_backend: trait bridge

```python title="Python"
from xberg import register_embedding_backend, unregister_embedding_backend, ExtractionConfig

def main() -> None:
    class _TestStub_register_embedding_backend_trait_bridge:
        def name(self):
            return "test-embedding-backend"
        def initialize(self):
            pass
        def shutdown(self):
            pass
        def dimensions(self):
            return 1
        async def embed(self, _p0):
            return []
    _ = register_embedding_backend(_TestStub_register_embedding_backend_trait_bridge())
    unregister_embedding_backend("test-embedding-backend")

main()

```
