---
id: fixture_php_unregister_post_processor_after_register
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

unregister_post_processor

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
Xberg::unregisterPostProcessor("test-processor");

```
