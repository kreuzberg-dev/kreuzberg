---
id: fixture_php_url_remote_text_document
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com"]));
$result = Xberg::extract($input, ["url" => ["mode" => "document"]]);
var_dump($result);

```
