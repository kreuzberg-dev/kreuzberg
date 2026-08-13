```go title="Go"
package main

import (
	"fmt"
	"log"

	"github.com/xberg-io/xberg/packages/go"
)

func main() {
	input := xberg.ExtractInputFromURI("document.pdf")
	result, err := xberg.Extract(*input, xberg.ExtractionConfig{})
	if err != nil {
		log.Fatalf("extract failed: %v", err)
	}

	fmt.Println("Content:")
	fmt.Println(result.Results[0].Content)

	fmt.Println("\nMetadata:")
	if result.Results[0].Metadata != nil {
		fmt.Printf("Title: %v\n", result.Results[0].Metadata["title"])
		fmt.Printf("Author: %v\n", result.Results[0].Metadata["author"])
	}

	fmt.Printf("\nTables found: %d\n", len(result.Results[0].Tables))
	fmt.Printf("Images found: %d\n", len(result.Results[0].Images))
}
```
