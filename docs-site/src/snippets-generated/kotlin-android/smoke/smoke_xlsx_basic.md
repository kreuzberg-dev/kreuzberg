---
id: fixture_kotlin_android_smoke_xlsx_basic
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\",\"uri\":\"https://example.com/xlsx/stanley_cups.xlsx\"}", ExtractionConfig::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
