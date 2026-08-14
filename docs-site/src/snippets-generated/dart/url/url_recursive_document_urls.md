---
id: fixture_dart_url_recursive_document_urls
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com"}');
    final _config = await createExtractionConfigFromJson(json: '{"url":{"crawl":{"document_url_depth":1,"follow_document_urls":true,"respect_robots_txt":false},"mode":"document"}}');
    final result = await XbergBridge.extract(_input, config: _config);
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
