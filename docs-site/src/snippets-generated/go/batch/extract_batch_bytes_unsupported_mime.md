---
id: fixture_go_extract_batch_bytes_unsupported_mime
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```go title="Go"
package main

import (
	"encoding/json"
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	var inputs []xberg.ExtractInput
	if err := json.Unmarshal([]byte(`[{"bytes":"ZGF0YQ==","kind":"bytes","mime_type":"application/x-unknown"}]`), &inputs); err != nil {
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
