---
id: fixture_kotlin_android_output_format_markdown
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"output_format\":\"markdown\"}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
