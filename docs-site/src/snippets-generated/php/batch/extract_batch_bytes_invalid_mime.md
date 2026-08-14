---
id: fixture_php_extract_batch_bytes_invalid_mime
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$result = Xberg::extractBatch([ExtractInput::from_json('{"bytes":[72,101,108,108,111],"kind":"bytes","mime_type":"application/x-nonexistent"}')], \Xberg\ExtractionConfig::from_json('{}'));
var_dump($result);

```
