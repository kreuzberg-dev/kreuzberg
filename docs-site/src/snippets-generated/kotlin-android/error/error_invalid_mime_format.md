---
id: fixture_kotlin_android_error_invalid_mime_format
language: kotlin
target: kotlin_android
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with invalid MIME type format

```kotlin title="Kotlin (Android)"
import io.xberg.*
import com.fasterxml.jackson.module.kotlin.jacksonObjectMapper

fun main() = kotlinx.coroutines.runBlocking {
    val mapper = jacksonObjectMapper()
    try {
    val inputFile0 = java.util.Base64.getEncoder().encodeToString(java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/text/plain.txt")))
    val input = mapper.readValue("{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{},\"filename\":\"plain.txt\",\"kind\":\"bytes\",\"mime_type\":\"not-a-mime\"}".replace("__ALEF_DOC_FILE_0__", inputFile0), ExtractionConfig::class.java)
    val config = mapper.readValue("{}", ExtractionConfig::class.java)
    val result = Xberg.extract(input, config)
    } catch (error: Exception) {
        System.err.println("Call failed as expected: ${error.message}")
        return@runBlocking    }
    throw AssertionError("expected call to fail")
}

```
