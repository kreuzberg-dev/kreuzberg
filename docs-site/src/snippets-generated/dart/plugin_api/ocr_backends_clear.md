---
id: fixture_dart_ocr_backends_clear
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

Clear all OCR backends and verify list is empty

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.clearOcrBackends();
  } finally {
    RustLib.dispose();
  }
}

```
