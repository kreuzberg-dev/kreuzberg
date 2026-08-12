---
id: fixture_kotlin_android_code_shebang_detection
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val input = mapper.readValue("{\"kind\":\"uri\",\"mime_type\":\"text/x-source-code\",\"uri\":\"https://example.com/code/script.sh\"}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
