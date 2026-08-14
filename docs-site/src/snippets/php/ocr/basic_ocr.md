```php title="PHP"
<?php

declare(strict_types=1);

/**
 * Basic OCR with Tesseract
 *
 * Extract text from scanned PDFs and images using Tesseract OCR.
 */

require_once __DIR__ . '/vendor/autoload.php';

use Xberg\ExtractionConfig;
use Xberg\OcrConfig;

$config = new ExtractionConfig(
    ocr: new OcrConfig(
        backend: 'tesseract',
        language: 'eng'
    )
);

$output = \Xberg\XbergApi::extract(\Xberg\ExtractInput::fromUri('scanned_document.pdf'), $config ?? \Xberg\ExtractionConfig::default());
$result = $output->getResults()[0];

echo "OCR Extraction Results:\n";
echo str_repeat('=', 60) . "\n";
echo $result->content . "\n\n";

$multilingualConfig = new ExtractionConfig(
    ocr: new OcrConfig(
        backend: 'tesseract',
        language: 'eng+fra+deu'  
    )
);

$output = \Xberg\XbergApi::extract(\Xberg\ExtractInput::fromUri('multilingual_scan.pdf'), $config ?? \Xberg\ExtractionConfig::default());
$result = $output->getResults()[0];

echo "Multilingual OCR:\n";
echo str_repeat('=', 60) . "\n";
echo substr($result->content, 0, 500) . "...\n\n";

$imageConfig = new ExtractionConfig(
    ocr: new OcrConfig(
        backend: 'tesseract',
        language: 'eng'
    )
);


$imageFormats = ['png', 'jpg', 'tiff'];
foreach ($imageFormats as $format) {
    $file = "scan.$format";
    if (file_exists($file)) {
        echo "Processing $file...\n";
        $output = \Xberg\XbergApi::extract(\Xberg\ExtractInput::fromUri($file), $config ?? \Xberg\ExtractionConfig::default());
$result = $output->getResults()[0];
        echo "Extracted " . strlen($result->content) . " characters\n";
        echo "Preview: " . substr($result->content, 0, 100) . "...\n\n";
    }
}

$languages = [
    'spa' => 'Spanish document',
    'fra' => 'French document',
    'deu' => 'German document',
    'ita' => 'Italian document',
    'por' => 'Portuguese document',
    'rus' => 'Russian document',
    'jpn' => 'Japanese document',
    'chi_sim' => 'Chinese (Simplified) document',
];

foreach ($languages as $lang => $description) {
    $file = strtolower(str_replace(' ', '_', $description)) . '.pdf';

    if (file_exists($file)) {
        $config = new ExtractionConfig(
            ocr: new OcrConfig(
                backend: 'tesseract',
                language: $lang
            )
        );

        $output = \Xberg\XbergApi::extract(\Xberg\ExtractInput::fromUri($file), $config ?? \Xberg\ExtractionConfig::default());
$result = $output->getResults()[0];

        echo "$description ($lang):\n";
        echo "  Characters extracted: " . mb_strlen($result->content) . "\n\n";
    }
}


$config = new ExtractionConfig(
    ocr: new OcrConfig(backend: 'tesseract', language: 'eng')
);

$output = \Xberg\XbergApi::extract(\Xberg\ExtractInput::fromUri('invoice_scan.pdf'), $config);
$result = $output->getResults()[0];

echo "Invoice OCR:\n";
echo str_repeat('=', 60) . "\n";
echo $result->content . "\n";

$output = \Xberg\XbergApi::extract(\Xberg\ExtractInput::fromUri('scanned.pdf'), $config ?? \Xberg\ExtractionConfig::default());
$result = $output->getResults()[0];

$contentLength = strlen($result->content);
$pageCount = $result->metadata?->pdf?->page_count ?? 1;
$avgCharsPerPage = $contentLength / $pageCount;

echo "\nOCR Quality Assessment:\n";
echo "Total characters: $contentLength\n";
echo "Pages: $pageCount\n";
echo "Average chars/page: " . number_format($avgCharsPerPage) . "\n";

if ($avgCharsPerPage < 100) {
    echo "Warning: Low character count may indicate poor scan quality\n";
    echo "Consider using image preprocessing or higher DPI settings.\n";
} elseif ($avgCharsPerPage > 2000) {
    echo "Pass: Good - Adequate text extracted\n";
} else {
    echo "Pass: Moderate - Text extracted successfully\n";
}
```
