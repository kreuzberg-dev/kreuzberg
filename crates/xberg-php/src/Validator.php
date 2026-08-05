<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for Validator.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface Validator
{
    // Optional methods the bridge calls when the class defines them (the
    // trait's Rust default behavior applies otherwise): should_validate, priority.
    // The lifecycle hooks initialize()/shutdown() are likewise optional.
    /**
     * Validate an extraction result.
     *
     *
     * @param ExtractedDocument $result
     * @param ExtractionConfig $config
     * @return mixed Return value from the plugin method
     */
    public function validate(ExtractedDocument $result, ExtractionConfig $config): mixed;
}
