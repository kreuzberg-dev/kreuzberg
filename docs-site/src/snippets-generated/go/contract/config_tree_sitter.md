---
id: fixture_go_config_tree_sitter
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

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
		URI:  ptr(`https://example.com/code/hello.py`),
	}
	config := xberg.ExtractionConfig{
		TreeSitter: &xberg.TreeSitterConfig{
		Process: &xberg.TreeSitterProcessConfig{
		Structure:   true,
		Imports:     true,
		Exports:     true,
		Comments:    false,
		Docstrings:  false,
		Symbols:     false,
		Diagnostics: false,
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
