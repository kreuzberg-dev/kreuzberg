---
id: fixture_kotlin_android_error_empty_bytes
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"bytes\":[],\"config\":{},\"filename\":\"empty.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
