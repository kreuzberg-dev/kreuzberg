---
id: fixture_go_api_extract_batch_bytes
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction API (extract_batch)

```go title="Go"
package main

import (
	"encoding/json"
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func main() {
	var inputs []xberg.ExtractInput
	if err := json.Unmarshal([]byte(`[{"bytes":"test_documents/pdf/fake_memo.pdf","filename":"fake_memo.pdf","kind":"bytes"}]`), &inputs); err != nil {
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
