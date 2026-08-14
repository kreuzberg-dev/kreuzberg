---
id: fixture_php_register_validator_trait_bridge
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

register_validator: trait bridge

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\Validator;
$stub = new class implements \Xberg\Validator {
    public function name(): string { return 'test-validator'; }
    public function validate($result, $config): mixed { return null; }
    public function should_validate($_result, $_config): bool { return false; }
    public function priority(): int { return 1; }
    public function version(): string { return ''; }
    public function initialize(): mixed { return null; }
    public function shutdown(): mixed { return null; }
    public function description(): string { return ''; }
    public function author(): string { return ''; }
};
Xberg::registerValidator($stub);

```
