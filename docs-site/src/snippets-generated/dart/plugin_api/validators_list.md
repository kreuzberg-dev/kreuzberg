---
id: fixture_dart_validators_list
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

List all registered validators

```dart title="Dart"
import 'dart:io';
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.listValidators();
    stdout.writeln(result);
  } finally {
    RustLib.dispose();
  }
}

```
