---
id: fixture_dart_renderers_clear
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

Clear all renderers and verify list is empty

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.clearRenderers();
  } finally {
    RustLib.dispose();
  }
}

```
