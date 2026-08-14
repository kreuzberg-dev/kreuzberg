---
id: fixture_dart_extract_bytes_input_invalid_mime
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

extract bytes input with unsupported MIME type

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    try {
      final _input = await createExtractInputFromJson(json: '{"bytes":"test_documents/text/plain.txt","config":{},"filename":"plain.txt","kind":"bytes","mime_type":"application/x-nonexistent"}');
      final _config = await createExtractionConfigFromJson(json: '{}');
      final result = await XbergBridge.extract(_input, config: _config);
    } on XbergError catch (error) {
      stderr.writeln('${error.runtimeType}: $error');
    }
  } finally {
    RustLib.dispose();
  }
}

```
