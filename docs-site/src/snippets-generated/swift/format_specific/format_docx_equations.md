---
id: fixture_swift_format_docx_equations
language: swift
target: swift
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```swift title="Swift"
import Xberg

_ = try await Xberg.extract("{\"filename\":\"equations.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/equations.docx\"}", "{\"output_format\":\"markdown\"}")

```
