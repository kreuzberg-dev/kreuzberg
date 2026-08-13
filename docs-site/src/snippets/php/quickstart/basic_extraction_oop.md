```php title="PHP"
<?php

declare(strict_types=1);

/**
 * Basic Document Extraction (OOP API)
 *
 * This example demonstrates the simplest way to extract text from a document
 * using the object-oriented API.
 */

require_once __DIR__ . '/vendor/autoload.php';



$output = \Xberg\XbergApi::extract(\Xberg\ExtractInput::fromUri('document.pdf'), $config ?? \Xberg\ExtractionConfig::default());
$result = $output->getResults()[0];

echo "Extracted Content:\n";
echo "==================\n";
echo $result->content . "\n\n";

echo "Metadata:\n";
echo "=========\n";
echo "Title: " . ($result->metadata?->title ?? 'N/A') . "\n";
echo "Authors: " . (isset($result->metadata?->authors) ? implode(', ', $result->metadata?->authors) : 'N/A') . "\n";
echo "Pages: " . ($result->metadata?->pdf?->page_count ?? 'N/A') . "\n";
echo "Format: " . $result->mimeType . "\n\n";

if (count($result->tables) > 0) {
    echo "Tables Found: " . count($result->tables) . "\n";
    foreach ($result->tables as $index => $table) {
        echo "\nTable " . ($index + 1) . " (Page {$table->pageNumber}):\n";
        echo $table->markdown . "\n";
    }
}
```
