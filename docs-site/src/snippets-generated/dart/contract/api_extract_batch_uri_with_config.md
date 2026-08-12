---
id: fixture_dart_api_extract_batch_uri_with_config
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```dart title="Dart"
import 'dart:convert';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[{"config":{"output_format":"markdown"},"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final result = await XbergBridge.extractBatch(inputs);
  } finally {
    RustLib.dispose();
  }
}

```
