---
id: fixture_php_extract_bytes_input_invalid_mime
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with unsupported MIME type

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => "test_documents/text/plain.txt", "config" => [], "filename" => "plain.txt", "kind" => "bytes", "mimeType" => "application/x-nonexistent"]));
try {
    Xberg::extract($input, []);
} catch (Throwable $error) {
    echo "Call failed as expected: {$error->getMessage()}\n";
    return;
}
throw new RuntimeException('expected call to fail');

```
