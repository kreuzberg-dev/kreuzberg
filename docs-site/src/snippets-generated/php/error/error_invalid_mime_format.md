---
id: fixture_php_error_invalid_mime_format
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with invalid MIME type format

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => "test_documents/text/plain.txt", "config" => [], "filename" => "plain.txt", "kind" => "bytes", "mimeType" => "not-a-mime"]));
try {
    Xberg::extract($input, []);
} catch (Throwable $error) {
    echo $error::class . ': ' . $error->getMessage() . "\n";
}

```
