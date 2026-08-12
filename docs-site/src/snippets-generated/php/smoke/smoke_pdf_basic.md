---
id: fixture_php_smoke_pdf_basic
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "mimeType" => "application/pdf", "uri" => "https://example.com/pdf/fake_memo.pdf"]));
$result = Xberg::extract($input, []);

```
