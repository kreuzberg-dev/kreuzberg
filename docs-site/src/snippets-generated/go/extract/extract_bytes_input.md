---
id: fixture_go_extract_bytes_input
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

extract bytes input from PDF document

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
	}
	config := xberg.ExtractionConfig{}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
