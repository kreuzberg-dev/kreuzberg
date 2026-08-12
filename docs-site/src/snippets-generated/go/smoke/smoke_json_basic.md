---
id: fixture_go_smoke_json_basic
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func ptr[T any](value T) *T { return &value }
func main() {
	input := xberg.ExtractInput{
		Kind:     ptr(xberg.ExtractInputKind(`uri`)),
		URI:      ptr(`https://example.com/json/simple.json`),
		MimeType: ptr(`application/json`),
	}
	config := xberg.ExtractionConfig{}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
