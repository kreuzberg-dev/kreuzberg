```dart title="Dart"
import 'package:xberg/xberg.dart';

Future<void> main() async {
  final output = await XbergBridge.extract(
    const ExtractInput(
      kind: ExtractInputKind.uri,
      uri: 'document.pdf',
    ),
    await createExtractionConfigFromJson(json: '{}'),
  );

  print(output.results.first.content);
}
```
