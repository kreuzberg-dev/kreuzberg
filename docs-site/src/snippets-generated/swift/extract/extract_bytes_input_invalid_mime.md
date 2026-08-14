---
id: fixture_swift_extract_bytes_input_invalid_mime
language: swift
target: swift
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with unsupported MIME type

```swift title="Swift"
import Xberg

do {
    _ = try await Xberg.extract("{\"bytes\":\"test_documents/text/plain.txt\",\"config\":{},\"filename\":\"plain.txt\",\"kind\":\"bytes\",\"mime_type\":\"application/x-nonexistent\"}", "{}")
} catch {
    print("\(type(of: error)): \(error)")
}

```
