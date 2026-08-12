---
id: fixture_dart_extract_batch_bytes_happy
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```dart title="Dart"
import 'dart:convert';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[{"bytes":[72,101,108,108,111,44,32,119,111,114,108,100,33],"kind":"bytes","mime_type":"text/plain"},{"bytes":"test_documents/html/html.html","kind":"bytes","mime_type":"text/html"}]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final result = await XbergBridge.extractBatch(inputs);
  } finally {
    RustLib.dispose();
  }
}

```
