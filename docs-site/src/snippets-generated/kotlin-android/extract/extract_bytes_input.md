---
id: fixture_kotlin_android_extract_bytes_input
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    val inputFile0 = java.util.Base64.getEncoder().encodeToString(java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/pdf/fake_memo.pdf")))
    val input = mapper.readValue("{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\",\"mime_type\":\"application/pdf\"}".replace("__ALEF_DOC_FILE_0__", inputFile0), ExtractionConfig::class.java)
    val result = Xberg.extract(input, ExtractionConfig())
}

```
