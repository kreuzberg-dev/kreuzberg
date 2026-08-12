---
id: fixture_dart_list_reranker_backends
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.listRerankerBackends();
  } finally {
    RustLib.dispose();
  }
}

```
