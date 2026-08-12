---
id: fixture_go_config_chunking_prepend_heading_context
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func ptr[T any](value T) *T { return &value }
func main() {
	input := xberg.ExtractInput{
		Kind: ptr(xberg.ExtractInputKind(`uri`)),
		URI:  ptr(`document.md`),
	}
	config := xberg.ExtractionConfig{
		Chunking: &xberg.ChunkingConfig{
		MaxCharacters:         500,
		Overlap:               50,
		ChunkerType:           ptr(xberg.ChunkerType(`markdown`)),
		PrependHeadingContext: true,
	},
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
