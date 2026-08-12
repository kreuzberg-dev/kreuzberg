---
id: fixture_kotlin_android_clear_reranker_backends
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Clear all reranker backends and verify list is empty

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    RerankerBackendBridge.clearAll()
}

```
