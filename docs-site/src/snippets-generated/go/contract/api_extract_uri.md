---
id: fixture_go_api_extract_uri
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

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
	config := xberg.ExtractionConfig{}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
