---
id: fixture_php_config_document_structure_with_headings
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["kind" => "uri", "uri" => "https://example.com/docx/fake.docx"]));
$result = Xberg::extract($input, ["include_document_structure" => true]);

```
