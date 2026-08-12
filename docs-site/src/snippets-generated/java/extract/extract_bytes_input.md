---
id: fixture_java_extract_bytes_input
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputFile0 = java.util.Base64.getEncoder().encodeToString(
    java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/pdf/fake_memo.pdf"))
);
var inputJson = "{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\",\"mime_type\":\"application/pdf\"}";
inputJson = inputJson.replace("__ALEF_DOC_FILE_0__", inputFile0);
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var result = Xberg.extract(input, ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
