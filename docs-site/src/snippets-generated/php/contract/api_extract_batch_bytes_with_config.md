---
id: fixture_php_api_extract_batch_bytes_with_config
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$result = Xberg::extractBatch([ExtractInput::from_json('{"bytes":"test_documents/pdf/fake_memo.pdf","config":{"output_format":"markdown"},"filename":"fake_memo.pdf","kind":"bytes"}')], \Xberg\ExtractionConfig::from_json('{}'));

```
