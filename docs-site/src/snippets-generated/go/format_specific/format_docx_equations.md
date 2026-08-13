---
id: fixture_go_format_docx_equations
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```go title="Go"
package main

import (
	"fmt"
	xberg "github.com/xberg-io/xberg/packages/go"
)

func ptr[T any](value T) *T { return &value }
func main() {
	input := xberg.ExtractInput{
		Kind:     ptr(xberg.ExtractInputKind(`uri`)),
		URI:      ptr(`https://example.com/docx/equations.docx`),
		MimeType: ptr(`application/vnd.openxmlformats-officedocument.wordprocessingml.document`),
		Filename: ptr(`equations.docx`),
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
