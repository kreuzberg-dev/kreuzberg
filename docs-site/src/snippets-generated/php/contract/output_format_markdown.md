---
id: fixture_php_output_format_markdown
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/pdf/fake_memo.pdf"]));
$result = Xberg::extract($input, ["output_format" => "markdown"]);

```
