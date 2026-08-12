---
id: fixture_kotlin_android_unregister_embedding_backend_after_register
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

unregister_embedding_backend

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    EmbeddingBackendBridge.unregister("test-embedding-backend")
}

```
