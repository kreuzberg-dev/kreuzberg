---
id: fixture_csharp_register_embedding_backend_trait_bridge
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

register_embedding_backend: trait bridge

```csharp title="C#"
using Xberg;

XbergConverter.RegisterEmbeddingBackend(EmbeddingBackendBridge.Register(new TestStub_RegisterEmbeddingBackendTraitBridge()));
    private class TestStub_RegisterEmbeddingBackendTraitBridge : IEmbeddingBackend
    {
        public string Name => "register_embedding_backend_trait_bridge";
        public string Version => "1.0.0";

        public ulong Dimensions { get; } = 768;
        public List<List<float>> Embed(List<string> texts)
            => [];
        public void Initialize() { }
        public void Shutdown() { }
        public string Description { get; } = "";
        public string Author { get; } = "";
    }

```
