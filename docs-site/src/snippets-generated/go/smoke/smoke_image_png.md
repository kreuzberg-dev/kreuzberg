---
id: fixture_go_smoke_image_png
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

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
		URI:  ptr(`https://example.com/images/sample.png`),
	}
	config := xberg.ExtractionConfig{
		DisableOcr: true,
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
