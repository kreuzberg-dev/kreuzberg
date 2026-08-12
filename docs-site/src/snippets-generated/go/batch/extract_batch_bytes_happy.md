---
id: fixture_go_extract_batch_bytes_happy
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```go title="Go"
package main

import (
	"encoding/json"
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	var inputs []xberg.ExtractInput
	if err := json.Unmarshal([]byte(`[{"bytes":"SGVsbG8sIHdvcmxkIQ==","kind":"bytes","mime_type":"text/plain"},{"bytes":"test_documents/html/html.html","kind":"bytes","mime_type":"text/html"}]`), &inputs); err != nil {
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
