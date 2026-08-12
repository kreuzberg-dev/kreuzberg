---
id: fixture_kotlin_android_unregister_validator_after_register
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

unregister_validator

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    ValidatorBridge.unregister("test-validator")
}

```
