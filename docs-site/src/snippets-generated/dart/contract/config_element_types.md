---
id: fixture_dart_config_element_types
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/docx/unit_test_headers.docx"}');
    final _config = await createExtractionConfigFromJson(json: '{"result_format":"element_based"}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
