<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for DocumentExtractor.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface DocumentExtractor
{
    // Optional methods the bridge calls when the class defines them (the
    // trait's Rust default behavior applies otherwise): priority, can_handle.
    // The lifecycle hooks initialize()/shutdown() are likewise optional.
    /**
     * Binding-safe extraction entry point for foreign-language plugin bridges.
     *
     *
     * @param ExtractInput $input
     * @param ExtractionConfig $config
     * @return ExtractedDocument Return value from the plugin method
     */
    public function extract(ExtractInput $input, ExtractionConfig $config): ExtractedDocument;

    /**
     * Get the list of MIME types supported by this extractor.
     *
     *
     * @return mixed Return value from the plugin method
     */
    public function supported_mime_types(): mixed;
}
