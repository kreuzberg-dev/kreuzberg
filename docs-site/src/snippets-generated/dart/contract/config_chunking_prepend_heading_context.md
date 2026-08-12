---
id: fixture_dart_config_chunking_prepend_heading_context
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"document.md"}');
    final _config = await createExtractionConfigFromJson(json: '{"chunking":{"chunker_type":"markdown","max_characters":500,"overlap":50,"prepend_heading_context":true}}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
