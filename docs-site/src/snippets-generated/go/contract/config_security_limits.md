---
id: fixture_go_config_security_limits
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

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
		URI:  ptr(`https://example.com/archives/documents.zip`),
	}
	config := xberg.ExtractionConfig{
		SecurityLimits: &xberg.SecurityLimits{
		MaxArchiveSize:      104857600,
		MaxCompressionRatio: 50,
		MaxFilesInArchive:   100,
	},
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
