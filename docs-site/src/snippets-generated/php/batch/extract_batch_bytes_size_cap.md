---
id: fixture_php_extract_batch_bytes_size_cap
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$config = \Xberg\ExtractionConfig::from_json(json_encode(["securityLimits" => ["maxContentSize" => 1]]));
try {
    Xberg::extractBatch([ExtractInput::from_json('{"bytes":"test_documents/text/fake_text.txt","kind":"bytes","mime_type":"text/plain"}')], $config);
} catch (Throwable $error) {
    echo $error::class . ': ' . $error->getMessage() . "\n";
}

```
