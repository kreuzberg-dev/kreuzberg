---
id: fixture_kotlin_android_validators_clear
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Clear all validators and verify list is empty

```kotlin title="Kotlin (Android)"
import io.xberg.*

fun main() {
    ValidatorBridge.clearAll()
}

```
