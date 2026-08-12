---
id: fixture_kotlin_android_tokenizer_backends_clear
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Clear all tokenizer backends and verify list is empty

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    TokenizerBackendBridge.clearAll()
}

```
