---
id: fixture_go_error_extract_input_conflicting_ocr
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

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
		Bytes:    mustReadFile(`test_documents/text/fake_text.txt`),
		MimeType: ptr(`text/plain`),
		Filename: ptr(`fake_text.txt`),
		Config:   &xberg.FileExtractionConfig{
		ForceOcr:   true,
		DisableOcr: true,
	},
	}
	config := xberg.ExtractionConfig{
		ForceOcr:   true,
		DisableOcr: true,
	}
	_, err := xberg.Extract(input, config)
	if err == nil {
		panic("expected call to fail")
	}
	fmt.Fprintf(os.Stderr, "Call failed as expected: %v\n", err)
}
```
