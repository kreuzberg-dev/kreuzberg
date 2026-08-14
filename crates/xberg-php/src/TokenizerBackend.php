<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for TokenizerBackend.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface TokenizerBackend
{

    /**
     * Count the tokens in `text` according to this backend's tokenizer.
     *

     * @param string $text
     * @return int Return value from the plugin method
     */
    public function count_tokens(string $text): int;

}
