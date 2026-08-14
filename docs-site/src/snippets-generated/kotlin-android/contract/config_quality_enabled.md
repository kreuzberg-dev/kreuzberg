---
id: fixture_kotlin_android_config_quality_enabled
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"enable_quality_processing\":true}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
