---
id: fixture_kotlin_android_register_embedding_backend_trait_bridge
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

register_embedding_backend: trait bridge

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    class TestStubRegisterEmbeddingBackendTraitBridge : IEmbeddingBackend {
    override fun name(): String = "register_embedding_backend_trait_bridge"
    override fun dimensions(): Long = 768
    override suspend fun embed(texts: List<String>): List<List<Float>> = emptyList()
    override fun version(): String = ""
    override fun initialize(): Unit {}
    override fun shutdown(): Unit {}
    override fun description(): String = ""
    override fun author(): String = ""
}
// register via: EmbeddingBackendBridge.register(TestStubRegisterEmbeddingBackendTraitBridge())

    EmbeddingBackendBridge.register(TestStubRegisterEmbeddingBackendTraitBridge())
}

```
