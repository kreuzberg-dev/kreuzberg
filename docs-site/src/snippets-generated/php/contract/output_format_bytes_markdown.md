---
id: fixture_php_output_format_bytes_markdown
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["bytes" => "test_documents/pdf/fake_memo.pdf", "config" => ["outputFormat" => "markdown"], "filename" => "fake_memo.pdf", "kind" => "bytes", "mimeType" => "application/pdf"]));
$result = Xberg::extract($input, ["output_format" => "markdown"]);
var_dump($result);

```
