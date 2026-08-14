---
id: fixture_kotlin_android_config_element_types
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/docx/unit_test_headers.docx\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"result_format\":\"element_based\"}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
