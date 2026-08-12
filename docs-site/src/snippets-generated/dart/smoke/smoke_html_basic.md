---
id: fixture_dart_smoke_html_basic
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","mime_type":"text/html","uri":"https://example.com/html/simple_table.html"}');
    final _config = await createExtractionConfigFromJson(json: '{}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
