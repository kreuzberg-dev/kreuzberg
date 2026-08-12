---
id: fixture_go_url_remote_text_document
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

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
		URI:  ptr(`https://example.com`),
	}
	config := xberg.ExtractionConfig{
		URL: &xberg.UrlExtractionConfig{
		Mode: ptr(xberg.URLExtractionMode(`document`)),
	},
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
