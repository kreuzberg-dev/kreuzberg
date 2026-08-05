<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for PostProcessor.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface PostProcessor
{
    // Optional methods the bridge calls when the class defines them (the
    // trait's Rust default behavior applies otherwise): should_process, estimated_duration_ms, priority.
    // The lifecycle hooks initialize()/shutdown() are likewise optional.
    /**
     * Process an extraction result.
     *
     *
     * @param ExtractedDocument $result
     * @param ExtractionConfig $config
     * @return mixed Return value from the plugin method
     */
    public function process(ExtractedDocument $result, ExtractionConfig $config): mixed;

    /**
     * Get the processing stage for this post-processor.
     *
     *
     * @return mixed Return value from the plugin method
     */
    public function processing_stage(): mixed;
}
