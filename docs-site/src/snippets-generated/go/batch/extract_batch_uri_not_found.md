---
id: fixture_go_extract_batch_uri_not_found
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```go title="Go"
package main

import (
	"encoding/json"
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	var inputs []xberg.ExtractInput
	if err := json.Unmarshal([]byte(`[{"kind":"uri","uri":"/nonexistent/a.pdf"}]`), &inputs); err != nil {
		panic(fmt.Sprintf("config parse failed: %v", err))
	}
	config := xberg.ExtractionConfig{}
	result, err := xberg.ExtractBatch(inputs, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
