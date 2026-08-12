---
id: fixture_php_error_empty_mime
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => "test_documents/text/plain.txt", "config" => [], "filename" => "plain.txt", "kind" => "bytes", "mimeType" => ""]));
try {
    Xberg::extract($input, []);
} catch (Throwable $error) {
    echo "Call failed as expected: {$error->getMessage()}\n";
    return;
}
throw new RuntimeException('expected call to fail');

```
