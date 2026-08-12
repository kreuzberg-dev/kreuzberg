---
id: fixture_dart_url_crawl_linked_pages
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","uri":"https://example.com"}');
    final _config = await createExtractionConfigFromJson(json: '{"url":{"crawl":{"max_depth":1,"max_pages":4,"respect_robots_txt":false},"mode":"crawl"}}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
