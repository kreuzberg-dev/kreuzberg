---
id: fixture_go_url_crawl_linked_pages
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

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
		URI:  ptr(`https://example.com`),
	}
	config := xberg.ExtractionConfig{
		URL: &xberg.UrlExtractionConfig{
		Mode:  ptr(xberg.URLExtractionMode(`crawl`)),
		Crawl: &xberg.CrawlConfig{
		MaxDepth:         1,
		MaxPages:         4,
		RespectRobotsTxt: false,
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
