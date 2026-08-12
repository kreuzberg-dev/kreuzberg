---
id: fixture_go_output_format_bytes_markdown
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
	"os"
)

func ptr[T any](value T) *T { return &value }
func mustReadFile(path string) []byte {
	content, err := os.ReadFile(path)
	if err != nil {
		panic(err)
	}
	return content
}
func main() {
	input := xberg.ExtractInput{
		Kind:     ptr(xberg.ExtractInputKind(`bytes`)),
		Bytes:    mustReadFile(`test_documents/pdf/fake_memo.pdf`),
		MimeType: ptr(`application/pdf`),
		Filename: ptr(`fake_memo.pdf`),
		Config:   &xberg.FileExtractionConfig{
		OutputFormat: ptr(xberg.OutputFormat(`markdown`)),
	},
	}
	config := xberg.ExtractionConfig{
		OutputFormat: ptr(xberg.OutputFormat(`markdown`)),
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
