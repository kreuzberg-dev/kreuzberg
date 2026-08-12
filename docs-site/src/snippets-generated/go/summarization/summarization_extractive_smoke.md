---
id: fixture_go_summarization_extractive_smoke
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

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
		URI:  ptr(`https://example.com/text/book_war_and_peace_1p.txt`),
	}
	config := xberg.ExtractionConfig{
		Summarization: &xberg.SummarizationConfig{
		Strategy:  ptr(xberg.SummaryStrategy(`extractive`)),
		MaxTokens: 80,
	},
	}
	result, err := xberg.Extract(input, config)
	if err != nil {
		panic(err)
	}
	fmt.Printf("%+v\n", result)
}
```
