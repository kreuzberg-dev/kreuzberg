---
id: fixture_kotlin_android_embedding_backends_clear
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Clear all embedding backends and verify list is empty

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    EmbeddingBackendBridge.clearAll()
}

```
