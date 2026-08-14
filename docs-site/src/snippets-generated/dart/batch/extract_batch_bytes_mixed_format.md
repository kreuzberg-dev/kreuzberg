---
id: fixture_dart_extract_batch_bytes_mixed_format
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```dart title="Dart"
import 'dart:convert';
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[{"bytes":[80,68,70,32,112,108,97,99,101,104,111,108,100,101,114],"kind":"bytes","mime_type":"application/x-unknown"}]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final result = await XbergBridge.extractBatch(inputs);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
