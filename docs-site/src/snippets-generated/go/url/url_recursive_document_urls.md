---
id: fixture_go_url_recursive_document_urls
language: go
target: go
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

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
		Mode:  ptr(xberg.URLExtractionMode(`document`)),
		Crawl: &xberg.CrawlConfig{
		RespectRobotsTxt:   false,
		FollowDocumentUrls: true,
		DocumentURLDepth:   1,
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
