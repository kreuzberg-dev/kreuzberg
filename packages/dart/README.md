# xberg

High-performance document intelligence library

## Installation

Add to your `pubspec.yaml`:

```yaml
dependencies:
  xberg: ^1.1.0
```

Then run:

```sh
dart pub get
```

## Building

From the repository root:

```sh
cargo build -p xberg-dart
flutter_rust_bridge_codegen generate
dart pub get
dart analyze
dart test
```

## License

MIT
