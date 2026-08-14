---
id: fixture_kotlin_android_register_tokenizer_backend_trait_bridge
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

register_tokenizer_backend: trait bridge

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    class TestStubRegisterTokenizerBackendTraitBridge : ITokenizerBackend {
    override fun name(): String = "register_tokenizer_backend_trait_bridge"
    override fun countTokens(text: String): Long = 3
    override fun version(): String = ""
    override fun initialize(): Unit {}
    override fun shutdown(): Unit {}
    override fun description(): String = ""
    override fun author(): String = ""
}
// register via: TokenizerBackendBridge.register(TestStubRegisterTokenizerBackendTraitBridge())

    TokenizerBackendBridge.register(TestStubRegisterTokenizerBackendTraitBridge())
}

```
