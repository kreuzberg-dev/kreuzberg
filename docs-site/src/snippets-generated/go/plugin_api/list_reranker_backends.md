---
id: fixture_go_list_reranker_backends
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	result, err := xberg.ListRerankerBackends()
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
