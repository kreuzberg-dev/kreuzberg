---
id: fixture_php_error_extract_input_conflicting_ocr
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => "test_documents/text/fake_text.txt", "config" => ["disableOcr" => true, "forceOcr" => true], "filename" => "fake_text.txt", "kind" => "bytes", "mimeType" => "text/plain"]));
try {
    Xberg::extract($input, ["disable_ocr" => true, "force_ocr" => true]);
} catch (Throwable $error) {
    echo $error::class . ': ' . $error->getMessage() . "\n";
}

```
