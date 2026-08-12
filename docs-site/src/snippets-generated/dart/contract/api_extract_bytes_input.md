---
id: fixture_dart_api_extract_bytes_input
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"bytes":"test_documents/pdf/fake_memo.pdf","filename":"fake_memo.pdf","kind":"bytes"}');
    final _config = await createExtractionConfigFromJson(json: '{}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
