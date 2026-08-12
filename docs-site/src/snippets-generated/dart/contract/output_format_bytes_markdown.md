---
id: fixture_dart_output_format_bytes_markdown
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"bytes":"test_documents/pdf/fake_memo.pdf","config":{"output_format":"markdown"},"filename":"fake_memo.pdf","kind":"bytes","mime_type":"application/pdf"}');
    final _config = await createExtractionConfigFromJson(json: '{"output_format":"markdown"}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
