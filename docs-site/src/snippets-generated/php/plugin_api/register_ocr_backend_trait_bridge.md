---
id: fixture_php_register_ocr_backend_trait_bridge
language: php
target: php
level: typecheck
requires: []
side_effect: safe
---

register_ocr_backend: trait bridge

```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractedDocument;
use Xberg\OcrBackend;
$stub = new class implements \Xberg\OcrBackend {
    public function name(): string { return 'test-backend'; }
    public function process_image($image_bytes, $config): \Xberg\ExtractedDocument { return '{}'; }
    public function process_image_file($path, $config): \Xberg\ExtractedDocument { return '{}'; }
    public function supports_language($lang): bool { return false; }
    public function backend_type(): mixed { return '{}'; }
    public function supported_languages(): mixed { return []; }
    public function supports_table_detection(): bool { return false; }
    public function supports_document_processing(): bool { return false; }
    public function emits_structured_markdown(): bool { return false; }
    public function process_document($_path, $_config): \Xberg\ExtractedDocument { return '{}'; }
    public function version(): string { return ''; }
    public function initialize(): mixed { return null; }
    public function shutdown(): mixed { return null; }
    public function description(): string { return ''; }
    public function author(): string { return ''; }
};
Xberg::registerOcrBackend($stub);

```
