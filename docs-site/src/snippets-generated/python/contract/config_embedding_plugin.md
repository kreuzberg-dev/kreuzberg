---
id: fixture_python_config_embedding_plugin
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```python title="Python"
import asyncio

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/pdf/fake_memo.pdf")
    config = ExtractionConfig(chunking={"embedding": {"max_embed_duration_secs": 30, "model": {"name": "test-plugin-backend", "type": "plugin"}, "normalize": True}, "max_chars": 500, "max_overlap": 50})
    await extract(input, config)
    print(result)

asyncio.run(main())

```
