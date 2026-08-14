---
id: fixture_php_extract_batch_uri_partial_failure
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$result = Xberg::extractBatch([ExtractInput::from_json('{"kind":"uri","uri":"text/plain.txt"}'), ExtractInput::from_json('{"kind":"uri","uri":"/nonexistent/missing.pdf"}')], \Xberg\ExtractionConfig::from_json('{}'));
var_dump($result);

```
