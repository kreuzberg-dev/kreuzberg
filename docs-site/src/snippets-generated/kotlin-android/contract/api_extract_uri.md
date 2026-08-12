---
id: fixture_kotlin_android_api_extract_uri
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
