---
id: fixture_swift_error_invalid_mime_format
language: swift
target: swift
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with invalid MIME type format

```swift title="Swift"
import Xberg

do {
    _ = try await Xberg.extract("{\"bytes\":\"test_documents/text/plain.txt\",\"config\":{},\"filename\":\"plain.txt\",\"kind\":\"bytes\",\"mime_type\":\"not-a-mime\"}", "{}")
} catch {
    print("\(type(of: error)): \(error)")
}

```
