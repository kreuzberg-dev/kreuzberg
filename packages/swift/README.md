# Xberg

High-performance document intelligence library

## Installation

Add to your `Package.swift`:

```swift
.package(path: "packages/swift"),
```

## Building

```sh
cargo build -p xberg-swift
alef generate --lang swift
swift build --package-path packages/swift
swift test --package-path packages/swift
```

Before the Cargo build output exists, Alef emits placeholder RustBridge files so
the generated package layout is complete. After Cargo produces swift-bridge
artifacts, rerunning Alef replaces the placeholders with the generated Swift and
C bridge sources.

## License

MIT
