---
id: fixture_dart_output_format_markdown
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}');
    final _config = await createExtractionConfigFromJson(json: '{"output_format":"markdown"}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
