```swift title="Swift"
import Foundation
import Xberg
import RustBridge

let config = try extractionConfigFromJson("{}")
let input = try extractInputFromJson(#"{"kind":"uri","uri":"document.pdf"}"#)
let resultOutput = try await extract(input: input, config: config)
let result = resultOutput.results().get(index: 0)!

let tables = result.tables()
print("Tables: \(tables.count)")

for (index, table) in tables.enumerated() {
    print("Table \(index) on page \(table.pageNumber())")
    print(table.markdown().toString())

    if let bbox = table.boundingBox() {
        print("  Bounding box: \(bbox.x0()), \(bbox.y0()), \(bbox.x1()), \(bbox.y1())")
    }
}
```
