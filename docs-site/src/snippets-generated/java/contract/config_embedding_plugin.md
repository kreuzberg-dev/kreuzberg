---
id: fixture_java_config_embedding_plugin
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"chunking\":{\"embedding\":{\"max_embed_duration_secs\":30,\"model\":{\"name\":\"test-plugin-backend\",\"type\":\"plugin\"},\"normalize\":true},\"max_chars\":500,\"max_overlap\":50}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
