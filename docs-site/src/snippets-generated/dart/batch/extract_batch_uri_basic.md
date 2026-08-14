---
id: fixture_dart_extract_batch_uri_basic
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```dart title="Dart"
import 'dart:convert';
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[{"kind":"uri","uri":"pdf/fake_memo.pdf"},{"kind":"uri","uri":"text/fake_text.txt"}]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final result = await XbergBridge.extractBatch(inputs);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
