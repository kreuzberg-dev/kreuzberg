---
id: fixture_php_ocr_image_png
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => "test_documents/images/test_hello_world.png", "config" => [], "filename" => "test_hello_world.png", "kind" => "bytes", "mimeType" => "image/png"]));
$result = Xberg::extract($input, []);

```
