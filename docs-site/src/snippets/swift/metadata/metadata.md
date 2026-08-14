```swift title="Swift"
import Foundation
import Xberg
import RustBridge

let config = try extractionConfigFromJson("{}")
let input = try extractInputFromJson(#"{"kind":"uri","uri":"document.pdf"}"#)
let resultOutput = try await extract(input: input, config: config)
let result = resultOutput.results().get(index: 0)!

let metadata = result.metadata()

if let title = metadata.title() {
    print("Title: \(title.toString())")
}
if let subject = metadata.subject() {
    print("Subject: \(subject.toString())")
}
if let language = metadata.language() {
    print("Language: \(language.toString())")
}
if let createdAt = metadata.createdAt() {
    print("Created at: \(createdAt.toString())")
}
if let modifiedAt = metadata.modifiedAt() {
    print("Modified at: \(modifiedAt.toString())")
}
if let createdBy = metadata.createdBy() {
    print("Created by: \(createdBy.toString())")
}
// List-valued metadata crosses the bridge as a JSON array string.
print("Authors: \(metadata.authors().toString())")
print("Keywords: \(metadata.keywords().toString())")
if let duration = metadata.extractionDurationMs() {
    print("Extraction duration (ms): \(duration)")
}
if let pages = metadata.pages() {
    print("Page count: \(pages.totalCount())")
}
```
