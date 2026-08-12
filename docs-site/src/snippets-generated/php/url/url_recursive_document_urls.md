---
id: fixture_php_url_recursive_document_urls
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com"]));
$result = Xberg::extract($input, ["url" => ["crawl" => ["document_url_depth" => 1, "follow_document_urls" => true, "respect_robots_txt" => false], "mode" => "document"]]);

```
