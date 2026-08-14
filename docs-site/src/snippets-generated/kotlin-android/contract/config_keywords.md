---
id: fixture_kotlin_android_config_keywords
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput::class.java)
    val config = mapper.readValue("{\"keywords\":{\"algorithm\":\"yake\",\"max_keywords\":10}}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
}

```
