<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for EmbeddingBackend.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface EmbeddingBackend
{

    /**
     * Embedding vector dimension. Must be `> 0` and must match the length of
     *

     * @return int Return value from the plugin method
     */
    public function dimensions(): int;

    /**
     * Embed a batch of texts, returning one vector per input in order.
     *

     * @param mixed $texts
     * @return mixed Return value from the plugin method
     */
    public function embed(mixed $texts): mixed;

}
