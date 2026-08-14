---
id: fixture_php_format_hwpx_standalone
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["filename" => "simple.hwpx", "kind" => "uri", "mimeType" => "application/haansofthwpx", "uri" => "https://example.com/hwpx/simple.hwpx"]));
$result = Xberg::extract($input, null);
var_dump($result);

```
