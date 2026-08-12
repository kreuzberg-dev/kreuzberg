---
id: fixture_php_unregister_embedding_backend_after_register
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

unregister_embedding_backend

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
Xberg::unregisterEmbeddingBackend("test-embedding-backend");

```
