---
id: fixture_kotlin_android_config_document_structure_with_headings
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/docx/fake.docx\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"include_document_structure\":true}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
