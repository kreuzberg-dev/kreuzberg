---
id: fixture_php_list_ocr_backends
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

List OCR backends

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
$result = Xberg::listOcrBackends();

```
