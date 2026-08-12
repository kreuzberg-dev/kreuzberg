---
id: fixture_dart_unregister_validator_after_register
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

unregister_validator

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.unregisterValidator('test-validator');
  } finally {
    RustLib.dispose();
  }
}

```
