---
id: fixture_php_validators_clear
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

Clear all validators and verify list is empty

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
Xberg::clearValidators();

```
