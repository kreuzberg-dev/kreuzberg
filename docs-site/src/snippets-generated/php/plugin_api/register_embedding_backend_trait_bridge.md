---
id: fixture_php_register_embedding_backend_trait_bridge
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

register_embedding_backend: trait bridge

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\EmbeddingBackend;
$stub = new class implements \Xberg\EmbeddingBackend {
    public function name(): string { return 'test-embedding-backend'; }
    public function dimensions(): int { return 1; }
    public function embed($texts): mixed { return []; }
    public function version(): string { return ''; }
    public function initialize(): mixed { return null; }
    public function shutdown(): mixed { return null; }
    public function description(): string { return ''; }
    public function author(): string { return ''; }
};
Xberg::registerEmbeddingBackend($stub);

```
