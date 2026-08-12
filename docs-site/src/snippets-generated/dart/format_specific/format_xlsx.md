---
id: fixture_dart_format_xlsx
language: dart
target: dart
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```dart title="Dart"
import 'package:xberg/xberg.dart';
import 'package:xberg/src/xberg_bridge_generated/frb_generated.dart' show RustLib;
Future<void> main() async {
  await RustLib.init();
  try {
    final _input = await createExtractInputFromJson(json: '{"kind":"uri","mime_type":"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet","uri":"https://example.com/xlsx/stanley_cups.xlsx"}');
    final _config = await createExtractionConfigFromJson(json: '{}');
    final result = await XbergBridge.extract(_input, config: _config);
  } finally {
    RustLib.dispose();
  }
}

```
