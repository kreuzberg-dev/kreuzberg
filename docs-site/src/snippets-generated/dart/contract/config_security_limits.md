---
id: fixture_dart_config_security_limits
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/archives/documents.zip"}');
    final _config = await createExtractionConfigFromJson(json: '{"security_limits":{"max_archive_size":104857600,"max_compression_ratio":50,"max_files_in_archive":100}}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
