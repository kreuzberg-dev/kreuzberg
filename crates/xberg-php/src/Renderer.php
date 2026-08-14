<?php

declare(strict_types=1);

namespace Xberg;

/**
 * Plugin interface for Renderer.
 *
 * Implement this interface and register an instance with the corresponding
 * registration function to provide custom behavior for extraction.
 */
interface Renderer
{

    // Optional methods the bridge calls when the class defines them (the
    // trait's Rust default behavior applies otherwise): render_result.
    // The lifecycle hooks initialize()/shutdown() are likewise optional.
}
