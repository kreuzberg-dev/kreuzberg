---
id: fixture_kotlin_android_format_docx_standalone
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"filename\":\"fake.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/fake.docx\"}", ExtractInput::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
