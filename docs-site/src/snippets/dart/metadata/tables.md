```dart title="Dart"
import 'package:xberg/xberg.dart';

Future<void> main() async {
  final config = await createExtractionConfigFromJson(json: '{}');
  final result = await XbergBridge.extract(
    const ExtractInput(kind: ExtractInputKind.uri, uri: 'document.pdf'),
    config,
  );

  for (final table in result.results[0].tables) {
    print('Table on page ${table.pageNumber} with ${table.cells.length} rows');
    print(table.markdown);

    for (final row in table.cells) {
      print(row);
    }

    if (table.boundingBox != null) {
      print('Bounding box: ${table.boundingBox}');
    }
  }
}
```
