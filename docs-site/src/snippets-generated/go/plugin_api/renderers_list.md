---
id: fixture_go_renderers_list
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

List all registered renderers

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	result, err := xberg.ListRenderers()
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
