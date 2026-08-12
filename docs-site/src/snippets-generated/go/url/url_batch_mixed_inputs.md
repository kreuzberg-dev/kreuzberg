---
id: fixture_go_url_batch_mixed_inputs
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```go title="Go"
package main

import (
	"encoding/json"
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func ptr[T any](value T) *T { return &value }
func main() {
	var inputs []xberg.ExtractInput
	if err := json.Unmarshal([]byte(`[{"kind":"uri","uri":"https://example.com"},{"bytes":"QmF0Y2ggYnl0ZXMgY29udGVudA==","filename":"inline.txt","kind":"bytes","mime_type":"text/plain"}]`), &inputs); err != nil {
		panic(fmt.Sprintf("config parse failed: %v", err))
	}
	config := xberg.ExtractionConfig{
		URL: &xberg.UrlExtractionConfig{
		Mode: ptr(xberg.URLExtractionMode(`document`)),
	},
	}
	result, err := xberg.ExtractBatch(inputs, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
