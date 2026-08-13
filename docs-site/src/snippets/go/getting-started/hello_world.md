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

	fmt.Println("Extracted content:")
	if len(result.Results[0].Content) > 200 {
		fmt.Println(result.Results[0].Content[:200])
	} else {
		fmt.Println(result.Results[0].Content)
	}
}
```
