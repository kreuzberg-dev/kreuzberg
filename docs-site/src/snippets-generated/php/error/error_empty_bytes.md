---
id: fixture_php_error_empty_bytes
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => [], "config" => [], "filename" => "empty.txt", "kind" => "bytes", "mimeType" => "text/plain"]));
$result = Xberg::extract($input, []);
var_dump($result);

```
