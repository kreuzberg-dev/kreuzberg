---
id: fixture_php_config_pages
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/pdf/fake_memo.pdf"]));
$result = Xberg::extract($input, ["pages" => ["extract_pages" => true, "insert_page_markers" => true]]);

```
