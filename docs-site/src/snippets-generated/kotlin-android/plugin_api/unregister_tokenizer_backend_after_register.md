---
id: fixture_kotlin_android_unregister_tokenizer_backend_after_register
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

unregister_tokenizer_backend

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    TokenizerBackendBridge.unregister("test-tokenizer-backend")
}

```
