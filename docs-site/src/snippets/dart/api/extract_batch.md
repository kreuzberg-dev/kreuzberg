```dart title="Dart"
import 'dart:convert';
import 'package:xberg/xberg.dart';

Future<void> main() async {
  final output = await XbergBridge.extractBatch([
    const ExtractInput(
      kind: ExtractInputKind.uri,
      uri: 'document.pdf',
    ),
    ExtractInput(
      kind: ExtractInputKind.bytes,
      bytes: utf8.encode('Hello from memory'),
      mimeType: 'text/plain',
      filename: 'note.txt',
    ),
  ], await createExtractionConfigFromJson(json: '{}'));

  for (final result in output.results) {
    print(result.content);
  }
}
```
