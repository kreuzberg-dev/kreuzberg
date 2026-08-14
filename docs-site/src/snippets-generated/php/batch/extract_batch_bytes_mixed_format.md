---
id: fixture_php_extract_batch_bytes_mixed_format
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$result = Xberg::extractBatch([ExtractInput::from_json('{"bytes":[80,68,70,32,112,108,97,99,101,104,111,108,100,101,114],"kind":"bytes","mime_type":"application/x-unknown"}')], \Xberg\ExtractionConfig::from_json('{}'));
var_dump($result);

```
