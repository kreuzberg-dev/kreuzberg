---
id: fixture_php_url_crawl_linked_pages
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com"]));
$result = Xberg::extract($input, ["url" => ["crawl" => ["max_depth" => 1, "max_pages" => 4, "respect_robots_txt" => false], "mode" => "crawl"]]);
var_dump($result);

```
