---
id: fixture_php_list_validators
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

List validators

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
$result = Xberg::listValidators();
var_dump($result);

```
