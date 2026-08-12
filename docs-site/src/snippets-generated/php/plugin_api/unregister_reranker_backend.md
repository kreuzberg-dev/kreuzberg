---
id: fixture_php_unregister_reranker_backend
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

unregister_reranker_backend

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
Xberg::unregisterRerankerBackend("test-reranker-backend");

```
