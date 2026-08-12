---
id: fixture_go_format_hwpx_standalone
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

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
		URI:      ptr(`https://example.com/hwpx/simple.hwpx`),
		MimeType: ptr(`application/haansofthwpx`),
		Filename: ptr(`simple.hwpx`),
	}
	config := xberg.ExtractionConfig{}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
