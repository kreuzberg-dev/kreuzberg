---
id: fixture_dart_extract_batch_bytes_invalid_mime
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```dart title="Dart"
import 'dart:convert';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[{"bytes":[72,101,108,108,111],"kind":"bytes","mime_type":"application/x-nonexistent"}]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final result = await XbergBridge.extractBatch(inputs);
  } finally {
    RustLib.dispose();
  }
}

```
