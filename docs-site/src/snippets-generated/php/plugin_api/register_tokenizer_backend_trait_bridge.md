---
id: fixture_php_register_tokenizer_backend_trait_bridge
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

register_tokenizer_backend: trait bridge

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\TokenizerBackend;
$stub = new class implements \Xberg\TokenizerBackend {
    public function name(): string { return 'test-tokenizer-backend'; }
    public function count_tokens($text): int { return 1; }
    public function version(): string { return ''; }
    public function initialize(): mixed { return null; }
    public function shutdown(): mixed { return null; }
    public function description(): string { return ''; }
    public function author(): string { return ''; }
};
Xberg::registerTokenizerBackend($stub);

```
