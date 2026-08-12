---
id: fixture_kotlin_android_unregister_reranker_backend
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

unregister_reranker_backend

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    RerankerBackendBridge.unregister("test-reranker-backend")
}

```
