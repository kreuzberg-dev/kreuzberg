---
id: fixture_dart_config_pages
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}');
    final _config = await createExtractionConfigFromJson(json: '{"pages":{"extract_pages":true,"insert_page_markers":true}}');
    final result = await XbergBridge.extract(_input, config: _config);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
