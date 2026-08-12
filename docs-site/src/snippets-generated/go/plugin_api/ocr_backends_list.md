---
id: fixture_go_ocr_backends_list
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

List all registered OCR backends

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	result, err := xberg.ListOcrBackends()
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
