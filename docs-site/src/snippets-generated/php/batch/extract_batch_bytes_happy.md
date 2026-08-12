---
id: fixture_php_extract_batch_bytes_happy
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$result = Xberg::extractBatch([ExtractInput::from_json('{"bytes":[72,101,108,108,111,44,32,119,111,114,108,100,33],"kind":"bytes","mime_type":"text/plain"}'), ExtractInput::from_json('{"bytes":"test_documents/html/html.html","kind":"bytes","mime_type":"text/html"}')], \Xberg\ExtractionConfig::from_json('{}'));

```
