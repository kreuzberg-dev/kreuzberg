---
id: fixture_php_smoke_xlsx_basic
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "mimeType" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", "uri" => "https://example.com/xlsx/stanley_cups.xlsx"]));
$result = Xberg::extract($input, []);

```
