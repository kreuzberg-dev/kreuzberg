---
id: fixture_kotlin_android_format_docx_equations
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"filename\":\"equations.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/equations.docx\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{\"output_format\":\"markdown\"}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
