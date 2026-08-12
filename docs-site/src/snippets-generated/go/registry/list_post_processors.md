---
id: fixture_go_list_post_processors
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

List post-processors

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	result, err := xberg.ListPostProcessors()
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
