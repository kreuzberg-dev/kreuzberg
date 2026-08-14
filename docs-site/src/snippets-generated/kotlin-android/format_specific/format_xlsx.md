---
id: fixture_kotlin_android_format_xlsx
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\",\"uri\":\"https://example.com/xlsx/stanley_cups.xlsx\"}", ExtractInput::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
