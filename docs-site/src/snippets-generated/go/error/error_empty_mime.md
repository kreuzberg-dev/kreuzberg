---
id: fixture_go_error_empty_mime
language: go
target: go
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```go title="Go"
package main

import (
	"errors"
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
		Bytes:    mustReadFile(`test_documents/text/plain.txt`),
		MimeType: ptr(``),
		Filename: ptr(`plain.txt`),
		Config:   &xberg.FileExtractionConfig{},
	}
	config := xberg.ExtractionConfig{}
	_, err := xberg.Extract(input, config)
	var typedError *xberg.XbergError
	if errors.As(err, &typedError) {
		fmt.Fprintf(os.Stderr, "%T: %v\n", typedError, typedError)
	}
}
```
