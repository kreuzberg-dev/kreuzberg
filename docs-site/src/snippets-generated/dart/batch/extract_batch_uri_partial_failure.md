---
id: fixture_dart_extract_batch_uri_partial_failure
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```dart title="Dart"
import 'dart:convert';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[{"kind":"uri","uri":"text/plain.txt"},{"kind":"uri","uri":"/nonexistent/missing.pdf"}]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final result = await XbergBridge.extractBatch(inputs);
  } finally {
    RustLib.dispose();
  }
}

```
