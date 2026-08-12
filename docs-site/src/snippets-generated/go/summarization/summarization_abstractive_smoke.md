---
id: fixture_go_summarization_abstractive_smoke
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

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
		Strategy:  ptr(xberg.SummaryStrategy(`abstractive`)),
		MaxTokens: 150,
		Llm:       &xberg.LlmConfig{
		Model:       ptr(`openai/gpt-4o-mini`),
		Temperature: 0.0,
		MaxTokens:   200,
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
