---
id: fixture_php_url_batch_mixed_inputs
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;
$config = \Xberg\ExtractionConfig::from_json(json_encode(["url" => ["mode" => "document"]]));
$result = Xberg::extractBatch([ExtractInput::from_json('{"kind":"uri","uri":"https://example.com"}'), ExtractInput::from_json('{"bytes":[66,97,116,99,104,32,98,121,116,101,115,32,99,111,110,116,101,110,116],"filename":"inline.txt","kind":"bytes","mime_type":"text/plain"}')], $config);
var_dump($result);

```
