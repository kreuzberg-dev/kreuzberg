---
id: fixture_dart_config_quality_enabled
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}');
    final _config = await createExtractionConfigFromJson(json: '{"enable_quality_processing":true}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
