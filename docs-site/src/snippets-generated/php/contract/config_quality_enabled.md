---
id: fixture_php_config_quality_enabled
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/pdf/fake_memo.pdf"]));
$result = Xberg::extract($input, ["enable_quality_processing" => true]);

```
