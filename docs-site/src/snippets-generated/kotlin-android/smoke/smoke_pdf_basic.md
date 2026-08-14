---
id: fixture_kotlin_android_smoke_pdf_basic
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"application/pdf\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
