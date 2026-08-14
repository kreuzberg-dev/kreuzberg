---
id: fixture_php_smoke_html_basic
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "mimeType" => "text/html", "uri" => "https://example.com/html/simple_table.html"]));
$result = Xberg::extract($input, []);
var_dump($result);

```
