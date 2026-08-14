---
id: fixture_kotlin_android_register_reranker_backend_trait_bridge
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

register_reranker_backend: trait bridge

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    class TestStubRegisterRerankerBackendTraitBridge : IRerankerBackend {
    override fun name(): String = "register_reranker_backend_trait_bridge"
    override suspend fun rerank(query: String, documents: List<String>): List<Float> = emptyList()
    override fun version(): String = ""
    override fun initialize(): Unit {}
    override fun shutdown(): Unit {}
    override fun description(): String = ""
    override fun author(): String = ""
}
// register via: RerankerBackendBridge.register(TestStubRegisterRerankerBackendTraitBridge())

    RerankerBackendBridge.register(TestStubRegisterRerankerBackendTraitBridge())
}

```
