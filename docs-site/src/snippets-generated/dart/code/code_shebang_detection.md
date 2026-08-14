---
id: fixture_dart_code_shebang_detection
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","mime_type":"text/x-source-code","uri":"https://example.com/code/script.sh"}');
    final _config = await createExtractionConfigFromJson(json: '{}');
    final result = await XbergBridge.extract(_input, config: _config);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
