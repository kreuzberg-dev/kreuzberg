---
id: fixture_swift_error_extract_input_conflicting_ocr
language: swift
target: swift
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```swift title="Swift"
import Xberg

do {
    _ = try await Xberg.extract("{\"bytes\":\"test_documents/text/fake_text.txt\",\"config\":{\"disable_ocr\":true,\"force_ocr\":true},\"filename\":\"fake_text.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", "{\"disable_ocr\":true,\"force_ocr\":true}")
} catch {
    print("\(type(of: error)): \(error)")
}

```
