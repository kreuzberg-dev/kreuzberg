---
id: fixture_dart_extract_batch_empty_inputs
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```dart title="Dart"
import 'dart:convert';
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final result = await XbergBridge.extractBatch(inputs);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
