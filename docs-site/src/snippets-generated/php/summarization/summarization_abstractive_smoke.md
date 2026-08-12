---
id: fixture_php_summarization_abstractive_smoke
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/text/book_war_and_peace_1p.txt"]));
$result = Xberg::extract($input, ["summarization" => ["llm" => ["max_tokens" => 200, "model" => "openai/gpt-4o-mini", "temperature" => 0.0], "max_tokens" => 150, "strategy" => "abstractive"]]);

```
