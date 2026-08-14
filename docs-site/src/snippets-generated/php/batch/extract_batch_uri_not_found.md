---
id: fixture_php_extract_batch_uri_not_found
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$result = Xberg::extractBatch([ExtractInput::from_json('{"kind":"uri","uri":"/nonexistent/a.pdf"}')], \Xberg\ExtractionConfig::from_json('{}'));
var_dump($result);

```
