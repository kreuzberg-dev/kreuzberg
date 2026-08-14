---
id: fixture_kotlin_android_extract_bytes_input_empty_mime
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with empty MIME type

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    try {
    val inputFile0 = java.util.Base64.getEncoder().encodeToString(java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/text/plain.txt")))
    val input = mapper.readValue("{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{},\"filename\":\"plain.txt\",\"kind\":\"bytes\",\"mime_type\":\"\"}".replace("__ALEF_DOC_FILE_0__", inputFile0), ExtractInput::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
    } catch (error: Exception) {
        System.err.println("${error::class.simpleName}: ${error.message}")
    }
}

```
