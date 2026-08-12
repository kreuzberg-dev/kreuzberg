---
id: fixture_php_config_llm_structured_extraction
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/pdf/fake_memo.pdf"]));
$result = Xberg::extract($input, ["structured_extraction" => ["llm" => ["model" => "openai/gpt-4o"], "schema" => ["properties" => ["date" => ["type" => "string"], "summary" => ["type" => "string"], "title" => ["type" => "string"]], "required" => ["title"], "type" => "object"], "schema_name" => "memo_data"]]);

```
