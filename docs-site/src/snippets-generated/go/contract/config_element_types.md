---
id: fixture_go_config_element_types
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

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
		URI:  ptr(`https://example.com/docx/unit_test_headers.docx`),
	}
	config := xberg.ExtractionConfig{
		ResultFormat: ptr(xberg.ResultFormat(`element_based`)),
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
