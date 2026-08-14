---
id: fixture_dart_format_docx_equations
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"filename":"equations.docx","kind":"uri","mime_type":"application/vnd.openxmlformats-officedocument.wordprocessingml.document","uri":"https://example.com/docx/equations.docx"}');
    final _config = await createExtractionConfigFromJson(json: '{"output_format":"markdown"}');
    final result = await XbergBridge.extract(_input, config: _config);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
