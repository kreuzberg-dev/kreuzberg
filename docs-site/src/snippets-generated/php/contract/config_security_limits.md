---
id: fixture_php_config_security_limits
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/archives/documents.zip"]));
$result = Xberg::extract($input, ["security_limits" => ["max_archive_size" => 104857600, "max_compression_ratio" => 50, "max_files_in_archive" => 100]]);

```
