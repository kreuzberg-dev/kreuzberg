---
id: fixture_kotlin_android_config_chunking_prepend_heading_context
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"document.md\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"chunking\":{\"chunker_type\":\"markdown\",\"max_chars\":500,\"max_overlap\":50,\"prepend_heading_context\":true}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
