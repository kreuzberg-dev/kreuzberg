---
id: fixture_dart_summarization_abstractive_smoke
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

LLM-driven abstractive summary. Skipped automatically when XBERG_LLM_API_KEY (or OPENAI_API_KEY) is not set.

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/text/book_war_and_peace_1p.txt"}');
    final _config = await createExtractionConfigFromJson(json: '{"summarization":{"llm":{"max_tokens":200,"model":"openai/gpt-4o-mini","temperature":0.0},"max_tokens":150,"strategy":"abstractive"}}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
