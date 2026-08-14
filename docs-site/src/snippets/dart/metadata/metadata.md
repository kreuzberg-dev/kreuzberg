```dart title="Dart"
import 'package:xberg/xberg.dart';

Future<void> main() async {
  final config = await createExtractionConfigFromJson(json: '{}');
  final result = await XbergBridge.extract(
    const ExtractInput(kind: ExtractInputKind.uri, uri: 'document.pdf'),
    config,
  );

  final metadata = result.results.first.metadata;

  if (metadata.title != null) {
    print('Title: ${metadata.title}');
  }
  if (metadata.subject != null) {
    print('Subject: ${metadata.subject}');
  }
  if (metadata.authors != null) {
    print('Authors: ${metadata.authors!.join(', ')}');
  }
  if (metadata.keywords != null) {
    print('Keywords: ${metadata.keywords!.join(', ')}');
  }
  if (metadata.language != null) {
    print('Language: ${metadata.language}');
  }
  if (metadata.createdAt != null) {
    print('Created: ${metadata.createdAt}');
  }
  if (metadata.modifiedAt != null) {
    print('Modified: ${metadata.modifiedAt}');
  }
  if (metadata.extractionDurationMs != null) {
    print('Extraction took: ${metadata.extractionDurationMs} ms');
  }

  for (final entry in metadata.additional.entries) {
    print('Additional[${entry.key}]: ${entry.value}');
  }
}
```
