---
id: fixture_php_config_tree_sitter
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/code/hello.py"]));
$result = Xberg::extract($input, ["tree_sitter" => ["groups" => ["web"], "languages" => ["python", "rust"], "process" => ["comments" => false, "diagnostics" => false, "docstrings" => false, "exports" => true, "imports" => true, "structure" => true, "symbols" => false]]]);

```
