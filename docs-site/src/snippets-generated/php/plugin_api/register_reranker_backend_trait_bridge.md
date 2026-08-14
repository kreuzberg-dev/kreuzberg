---
id: fixture_php_register_reranker_backend_trait_bridge
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

register_reranker_backend: trait bridge

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\RerankerBackend;
$stub = new class implements \Xberg\RerankerBackend {
    public function name(): string { return 'test-reranker-backend'; }
    public function rerank($query, $documents): mixed { return []; }
    public function version(): string { return ''; }
    public function initialize(): mixed { return null; }
    public function shutdown(): mixed { return null; }
    public function description(): string { return ''; }
    public function author(): string { return ''; }
};
Xberg::registerRerankerBackend($stub);

```
