---
id: fixture_java_url_html_page_extract
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"url\":{\"mode\":\"document\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
