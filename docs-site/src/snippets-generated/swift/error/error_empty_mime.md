---
id: fixture_swift_error_empty_mime
language: swift
target: swift
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```swift title="Swift"
import Xberg

do {
    _ = try await Xberg.extract("{\"bytes\":\"test_documents/text/plain.txt\",\"config\":{},\"filename\":\"plain.txt\",\"kind\":\"bytes\",\"mime_type\":\"\"}", "{}")
} catch {
    print("\(type(of: error)): \(error)")
}

```
