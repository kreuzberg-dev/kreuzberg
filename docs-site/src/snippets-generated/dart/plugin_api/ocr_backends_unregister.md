---
id: fixture_dart_ocr_backends_unregister
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

Unregister nonexistent OCR backend gracefully

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.unregisterOcrBackend('nonexistent-backend-xyz');
  } finally {
    RustLib.dispose();
  }
}

```
