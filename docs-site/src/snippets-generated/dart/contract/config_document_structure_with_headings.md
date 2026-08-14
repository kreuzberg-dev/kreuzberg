---
id: fixture_dart_config_document_structure_with_headings
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/docx/fake.docx"}');
    final _config = await createExtractionConfigFromJson(json: '{"include_document_structure":true}');
    final result = await XbergBridge.extract(_input, config: _config);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
