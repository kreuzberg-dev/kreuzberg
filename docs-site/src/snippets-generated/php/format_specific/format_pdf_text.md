---
id: fixture_php_format_pdf_text
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["filename" => "fake_memo.pdf", "kind" => "uri", "mimeType" => "application/pdf", "uri" => "https://example.com/pdf/fake_memo.pdf"]));
$result = Xberg::extract($input, null);
var_dump($result);

```
