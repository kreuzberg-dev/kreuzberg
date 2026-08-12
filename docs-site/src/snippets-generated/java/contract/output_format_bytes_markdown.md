---
id: fixture_java_output_format_bytes_markdown
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputFile0 = java.util.Base64.getEncoder().encodeToString(
    java.nio.file.Files.readAllBytes(java.nio.file.Path.of("test_documents/pdf/fake_memo.pdf"))
);
var inputJson = "{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{\"output_format\":\"markdown\"},\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\",\"mime_type\":\"application/pdf\"}";
inputJson = inputJson.replace("__ALEF_DOC_FILE_0__", inputFile0);
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"output_format\":\"markdown\"}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
