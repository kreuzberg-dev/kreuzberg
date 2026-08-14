---
id: fixture_csharp_register_reranker_backend_trait_bridge
language: csharp
target: csharp
level: typecheck
requires: []
side_effect: safe
---

register_reranker_backend: trait bridge

```csharp title="C#"
using Xberg;

XbergConverter.RegisterRerankerBackend(RerankerBackendBridge.Register(new TestStub_RegisterRerankerBackendTraitBridge()));
    private class TestStub_RegisterRerankerBackendTraitBridge : IRerankerBackend
    {
        public string Name => "register_reranker_backend_trait_bridge";
        public string Version => "1.0.0";

        public List<float> Rerank(string query, List<string> documents)
            => [];
        public void Initialize() { }
        public void Shutdown() { }
        public string Description { get; } = "";
        public string Author { get; } = "";
    }

```
