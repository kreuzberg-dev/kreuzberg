---
id: fixture_kotlin_android_smoke_json_basic
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"application/json\",\"uri\":\"https://example.com/json/simple.json\"}", ExtractInput::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
