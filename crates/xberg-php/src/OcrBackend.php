<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for OcrBackend.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface OcrBackend
{
    // Optional methods the bridge calls when the class defines them (the
    // trait's Rust default behavior applies otherwise): process_image_file, supported_languages, supports_table_detection, supports_document_processing, emits_structured_markdown, process_document.
    // The lifecycle hooks initialize()/shutdown() are likewise optional.
    /**
     * Process an image and extract text via OCR.
     *
     *
     * @param mixed $image_bytes
     * @param OcrConfig $config
     * @return ExtractedDocument Return value from the plugin method
     */
    public function process_image(mixed $image_bytes, OcrConfig $config): ExtractedDocument;

    /**
     * Check if this backend supports a given language code.
     *
     *
     * @param string $lang
     * @return bool Return value from the plugin method
     */
    public function supports_language(string $lang): bool;

    /**
     * Get the backend type identifier.
     *
     *
     * @return mixed Return value from the plugin method
     */
    public function backend_type(): mixed;
}
