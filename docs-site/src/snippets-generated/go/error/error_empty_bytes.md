---
id: fixture_go_error_empty_bytes
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func ptr[T any](value T) *T { return &value }
func main() {
	input := xberg.ExtractInput{
		Kind:     ptr(xberg.ExtractInputKind(`bytes`)),
		Bytes:    []byte{},
		MimeType: ptr(`text/plain`),
		Filename: ptr(`empty.txt`),
		Config:   &xberg.FileExtractionConfig{},
	}
	config := xberg.ExtractionConfig{}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
