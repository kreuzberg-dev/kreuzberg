---
id: fixture_go_config_document_structure_with_headings
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

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
		URI:  ptr(`https://example.com/docx/fake.docx`),
	}
	config := xberg.ExtractionConfig{
		IncludeDocumentStructure: true,
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
