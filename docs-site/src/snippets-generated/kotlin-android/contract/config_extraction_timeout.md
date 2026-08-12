---
id: fixture_kotlin_android_config_extraction_timeout
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"extraction_timeout_secs\":300}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
