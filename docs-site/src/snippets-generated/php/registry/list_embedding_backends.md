---
id: fixture_php_list_embedding_backends
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

List embedding backends

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
$result = Xberg::listEmbeddingBackends();
var_dump($result);

```
