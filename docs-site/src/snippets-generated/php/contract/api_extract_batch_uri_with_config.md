---
id: fixture_php_api_extract_batch_uri_with_config
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$result = Xberg::extractBatch([ExtractInput::from_json('{"config":{"output_format":"markdown"},"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}')], \Xberg\ExtractionConfig::from_json('{}'));
var_dump($result);

```
