---
id: fixture_dart_error_invalid_mime_format
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with invalid MIME type format

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    try {
      final _input = await createExtractInputFromJson(json: '{"bytes":"test_documents/text/plain.txt","config":{},"filename":"plain.txt","kind":"bytes","mime_type":"not-a-mime"}');
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
