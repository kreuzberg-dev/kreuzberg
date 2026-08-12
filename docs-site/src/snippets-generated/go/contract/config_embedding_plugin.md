---
id: fixture_go_config_embedding_plugin
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

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
		URI:  ptr(`https://example.com/pdf/fake_memo.pdf`),
	}
	config := xberg.ExtractionConfig{
		Chunking: &xberg.ChunkingConfig{
		Embedding: &xberg.EmbeddingConfig{
		Model:                ptr(xberg.EmbeddingModelType(`{"name":"test-plugin-backend","type":"plugin"}`)),
		Normalize:            true,
		MaxEmbedDurationSecs: 30,
	},
	},
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
