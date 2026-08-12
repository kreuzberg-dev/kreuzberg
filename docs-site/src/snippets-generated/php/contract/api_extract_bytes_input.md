---
id: fixture_php_api_extract_bytes_input
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => "test_documents/pdf/fake_memo.pdf", "filename" => "fake_memo.pdf", "kind" => "bytes"]));
$result = Xberg::extract($input, null);

```
