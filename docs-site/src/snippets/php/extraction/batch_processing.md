```php title="PHP"
<?php

declare(strict_types=1);

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\Xberg;
use Xberg\ExtractInput;
use Xberg\ExtractionConfig;

$inputs = [
    ExtractInput::fromUri('document1.pdf'),
    ExtractInput::fromUri('document2.docx'),
    ExtractInput::fromBytes(file_get_contents('note.txt') ?: '', 'text/plain', 'note.txt'),
];

$config = new ExtractionConfig(
    extractTables: true,
    extractImages: false,
);

$output = Xberg::extractBatch($inputs, $config);

echo "Processed {$output->summary->results} documents\n";

foreach ($output->getResults() as $result) {
    echo "Content: " . strlen($result->content) . " chars\n";
    echo "Tables: " . count($result->tables) . "\n";
    echo "MIME: {$result->mimeType}\n\n";
}
```
