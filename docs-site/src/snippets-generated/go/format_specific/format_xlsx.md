---
id: fixture_go_format_xlsx
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

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
		URI:      ptr(`https://example.com/xlsx/stanley_cups.xlsx`),
		MimeType: ptr(`application/vnd.openxmlformats-officedocument.spreadsheetml.sheet`),
	}
	config := xberg.ExtractionConfig{}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
