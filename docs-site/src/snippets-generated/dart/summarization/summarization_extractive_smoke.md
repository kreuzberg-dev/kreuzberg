---
id: fixture_dart_summarization_extractive_smoke
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com/text/book_war_and_peace_1p.txt"}');
    final _config = await createExtractionConfigFromJson(json: '{"summarization":{"max_tokens":80,"strategy":"extractive"}}');
    final result = await XbergBridge.extract(_input, config: _config);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
