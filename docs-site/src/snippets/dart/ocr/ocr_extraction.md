```dart title="Dart"
import 'package:xberg/xberg.dart';

Future<void> main() async {
  // `ExtractionConfig` is a generated data class with no defaults, so build it
  // from JSON: every field you omit keeps its Rust-side default value.
  final config = await createExtractionConfigFromJson(json: '''
{
  "force_ocr": true,
  "ocr": {
    "backend": "tesseract",
    "language": ["eng"]
  }
}
''');

  const input = ExtractInput(
    kind: ExtractInputKind.uri,
    uri: 'scanned.pdf',
  );
  final output = await XbergBridge.extract(input, config);
  final document = output.results.first;

  print(document.content);
}
```
