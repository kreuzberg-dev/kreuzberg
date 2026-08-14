---
id: fixture_php_config_embedding_plugin
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/pdf/fake_memo.pdf"]));
$result = Xberg::extract($input, ["chunking" => ["embedding" => ["max_embed_duration_secs" => 30, "model" => ["name" => "test-plugin-backend", "type" => "plugin"], "normalize" => true], "max_chars" => 500, "max_overlap" => 50]]);
var_dump($result);

```
