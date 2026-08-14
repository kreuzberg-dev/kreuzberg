---
id: fixture_php_format_docx_equations
language: php
target: php
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
$input = \Xberg\ExtractInput::from_json(json_encode(["filename" => "equations.docx", "kind" => "uri", "mimeType" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "uri" => "https://example.com/docx/equations.docx"]));
$result = Xberg::extract($input, ["output_format" => "markdown"]);
var_dump($result);

```
