```swift title="Swift"
import Foundation
import Xberg
import RustBridge

// The Swift binding throws `RustString` (not `XbergError`) for every
// failure surfaced from the Rust core. The string preserves the original
// error variant name and message (e.g. "UnsupportedFormat: ...",
// "MissingDependency: ...", "Parsing: ...") so callers can pattern-match
// on the prefix or simply print the message.
do {
    let config = try extractionConfigFromJson("{}")
    let input = try extractInputFromJson(#"{"kind":"uri","uri":"document.pdf"}"#)
    let resultOutput = try await extract(input: input, config: config)
    let result = resultOutput.results().get(index: 0)!
    print(result.content().toString())
} catch let error as RustString {
    let message = error.toString()
    if message.contains("UnsupportedFormat") {
        print("Unsupported format: \(message)")
    } else if message.contains("MissingDependency") {
        print("Install the required dependency: \(message)")
    } else if message.contains("Parsing") {
        print("Corrupt or invalid document: \(message)")
    } else if message.contains("Io") {
        print("File error: \(message)")
    } else {
        print("Extraction failed: \(message)")
    }
} catch {
    print("Unexpected error: \(error)")
}
```
