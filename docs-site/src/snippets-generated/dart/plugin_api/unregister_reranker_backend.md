---
id: fixture_dart_unregister_reranker_backend
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

unregister_reranker_backend

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.unregisterRerankerBackend('test-reranker-backend');
  } finally {
    RustLib.dispose();
  }
}

```
