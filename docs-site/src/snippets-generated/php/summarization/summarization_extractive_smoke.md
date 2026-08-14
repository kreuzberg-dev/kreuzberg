---
id: fixture_php_summarization_extractive_smoke
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/text/book_war_and_peace_1p.txt"]));
$result = Xberg::extract($input, ["summarization" => ["max_tokens" => 80, "strategy" => "extractive"]]);
var_dump($result);

```
