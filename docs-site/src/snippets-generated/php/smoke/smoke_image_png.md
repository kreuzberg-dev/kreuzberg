---
id: fixture_php_smoke_image_png
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/images/sample.png"]));
$result = Xberg::extract($input, ["disable_ocr" => true]);

```
