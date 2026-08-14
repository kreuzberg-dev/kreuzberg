---
id: fixture_php_config_chunking_prepend_heading_context
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "document.md"]));
$result = Xberg::extract($input, ["chunking" => ["chunker_type" => "markdown", "max_characters" => 500, "overlap" => 50, "prepend_heading_context" => true]]);
var_dump($result);

```
