---
id: fixture_dart_list_post_processors
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

List post-processors

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.listPostProcessors();
  } finally {
    RustLib.dispose();
  }
}

```
