---
id: fixture_php_code_shebang_detection
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "mimeType" => "text/x-source-code", "uri" => "https://example.com/code/script.sh"]));
$result = Xberg::extract($input, null);

```
