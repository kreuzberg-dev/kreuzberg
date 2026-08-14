---
id: fixture_dart_format_docx_standalone
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"filename":"fake.docx","kind":"uri","mime_type":"application/vnd.openxmlformats-officedocument.wordprocessingml.document","uri":"https://example.com/docx/fake.docx"}');
    final _config = await createExtractionConfigFromJson(json: '{}');
    final result = await XbergBridge.extract(_input, config: _config);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
