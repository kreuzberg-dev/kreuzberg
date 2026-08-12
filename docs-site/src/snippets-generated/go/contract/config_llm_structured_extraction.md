---
id: fixture_go_config_llm_structured_extraction
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func ptr[T any](value T) *T { return &value }
func main() {
	input := xberg.ExtractInput{
		Kind: ptr(xberg.ExtractInputKind(`uri`)),
		URI:  ptr(`https://example.com/pdf/fake_memo.pdf`),
	}
	config := xberg.ExtractionConfig{
		StructuredExtraction: &xberg.StructuredExtractionConfig{
		SchemaName: ptr(`memo_data`),
		Llm:        xberg.LlmConfig{
		Model: ptr(`openai/gpt-4o`),
	},
	},
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
