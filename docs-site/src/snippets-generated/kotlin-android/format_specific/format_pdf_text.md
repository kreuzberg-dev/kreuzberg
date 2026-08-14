---
id: fixture_kotlin_android_format_pdf_text
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"filename\":\"fake_memo.pdf\",\"kind\":\"uri\",\"mime_type\":\"application/pdf\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
