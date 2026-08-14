---
id: fixture_dart_register_post_processor_trait_bridge
language: dart
target: dart
level: typecheck
requires: []
side_effect: safe
---

register_post_processor: trait bridge

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final result = await XbergBridge.registerPostProcessor(await _createTestStubRegisterPostProcessorTraitBridgeWrapper());
  } finally {
    RustLib.dispose();
  }
}

```
