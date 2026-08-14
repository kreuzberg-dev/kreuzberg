---
id: fixture_dart_embedding_backends_list
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

List all registered embedding backends

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.listEmbeddingBackends();
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
