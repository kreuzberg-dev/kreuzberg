---
id: fixture_go_format_pptx
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

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
		URI:      ptr(`https://example.com/pptx/simple.pptx`),
		MimeType: ptr(`application/vnd.openxmlformats-officedocument.presentationml.presentation`),
	}
	config := xberg.ExtractionConfig{}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
