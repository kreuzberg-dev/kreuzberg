---
id: fixture_dart_unregister_post_processor_after_register
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

unregister_post_processor

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.unregisterPostProcessor('test-processor');
  } finally {
    RustLib.dispose();
  }
}

```
