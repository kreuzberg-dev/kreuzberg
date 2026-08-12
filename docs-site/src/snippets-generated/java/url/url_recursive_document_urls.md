---
id: fixture_java_url_recursive_document_urls
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"url\":{\"crawl\":{\"document_url_depth\":1,\"follow_document_urls\":true,\"respect_robots_txt\":false},\"mode\":\"document\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
