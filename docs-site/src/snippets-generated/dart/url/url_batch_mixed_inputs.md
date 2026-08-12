---
id: fixture_dart_url_batch_mixed_inputs
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```dart title="Dart"
import 'dart:convert';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final inputs = await Future.wait((jsonDecode(r'[{"kind":"uri","uri":"https://example.com"},{"bytes":[66,97,116,99,104,32,98,121,116,101,115,32,99,111,110,116,101,110,116],"filename":"inline.txt","kind":"bytes","mime_type":"text/plain"}]') as List<dynamic>).cast<Map<String, dynamic>>().map((m) => createExtractInputFromJson(json: jsonEncode(m))));
    final _config = await createExtractionConfigFromJson(json: '{"url":{"mode":"document"}}');
    final result = await XbergBridge.extractBatch(inputs, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
