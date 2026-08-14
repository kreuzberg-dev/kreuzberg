---
id: fixture_swift_extract_batch_bytes_size_cap
language: swift
target: swift
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```swift title="Swift"
import Xberg

do {
    let _item_inputsArray_0 = try Xberg.extractInputFromJson("{\"bytes\":\"test_documents/text/fake_text.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}")
    let inputsArray = [_item_inputsArray_0]
    let configObj = try Xberg.extractionConfigFromJson("{\"security_limits\":{\"max_content_size\":1}}")
    _ = try await Xberg.extractBatch(inputs: inputsArray, config: configObj)
} catch {
    print("\(type(of: error)): \(error)")
}

```
