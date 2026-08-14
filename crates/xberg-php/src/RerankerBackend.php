<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for RerankerBackend.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface RerankerBackend
{

    /**
     * Score a list of documents against a query.
     *

     * @param string $query
     * @param mixed $documents
     * @return mixed Return value from the plugin method
     */
    public function rerank(string $query, mixed $documents): mixed;

}
