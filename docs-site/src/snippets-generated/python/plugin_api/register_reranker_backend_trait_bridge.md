---
id: fixture_python_register_reranker_backend_trait_bridge
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

register_reranker_backend: trait bridge

```python title="Python"
from xberg import register_reranker_backend, unregister_reranker_backend, ExtractionConfig

def main() -> None:
    class _TestStub_register_reranker_backend_trait_bridge:
        def name(self):
            return "test-reranker-backend"
        def initialize(self):
            pass
        def shutdown(self):
            pass
        async def rerank(self, _p0, _p1):
            return []
    register_reranker_backend(_TestStub_register_reranker_backend_trait_bridge())
    unregister_reranker_backend("test-reranker-backend")

main()

```
