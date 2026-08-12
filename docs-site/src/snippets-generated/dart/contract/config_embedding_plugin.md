---
id: fixture_dart_config_embedding_plugin
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/pdf/fake_memo.pdf"}');
    final _config = await createExtractionConfigFromJson(json: '{"chunking":{"embedding":{"max_embed_duration_secs":30,"model":{"name":"test-plugin-backend","type":"plugin"},"normalize":true},"max_chars":500,"max_overlap":50}}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
