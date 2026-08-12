---
id: fixture_dart_config_tree_sitter
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/code/hello.py"}');
    final _config = await createExtractionConfigFromJson(json: '{"tree_sitter":{"groups":["web"],"languages":["python","rust"],"process":{"comments":false,"diagnostics":false,"docstrings":false,"exports":true,"imports":true,"structure":true,"symbols":false}}}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
