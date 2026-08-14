---
id: fixture_php_register_post_processor_trait_bridge
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

register_post_processor: trait bridge

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\PostProcessor;
$stub = new class implements \Xberg\PostProcessor {
    public function name(): string { return 'test-processor'; }
    public function process($result, $config): mixed { return null; }
    public function processing_stage(): mixed { return '{}'; }
    public function should_process($_result, $_config): bool { return false; }
    public function estimated_duration_ms($_result): int { return 1; }
    public function priority(): int { return 1; }
    public function version(): string { return ''; }
    public function initialize(): mixed { return null; }
    public function shutdown(): mixed { return null; }
    public function description(): string { return ''; }
    public function author(): string { return ''; }
};
Xberg::registerPostProcessor($stub);

```
