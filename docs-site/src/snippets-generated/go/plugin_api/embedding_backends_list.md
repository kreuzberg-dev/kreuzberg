---
id: fixture_go_embedding_backends_list
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

List all registered embedding backends

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	result, err := xberg.ListEmbeddingBackends()
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
