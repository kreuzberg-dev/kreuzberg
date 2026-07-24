// swift-tools-version: 6.0
// Root-level Package.swift — alef-generated for published distributions.
//
// This manifest uses `.binaryTarget` for pre-built XCFramework/artifact bundles.
// External consumers depend on this via `.package(url: "...", from: "...")`.
//
// For in-tree development, see `packages/swift/Package.swift` and
// `packages/swift/README.md` for the source-based workflow.
import PackageDescription

let package = Package(
  name: "Xberg",
  platforms: [
    .macOS(.v13),
    .iOS(.v16),
  ],
  products: [
    .library(name: "Xberg", targets: ["Xberg"])
  ],
  targets: [
    // RustBridgeC: C headers target. Swift files in RustBridge import this to
    // access C types (RustStr, etc.) produced by swift-bridge.
    // publicHeadersPath: "." exposes the headers.
    .target(
      name: "RustBridgeC",
      path: "packages/swift/Sources/RustBridgeC",
      publicHeadersPath: "."
    ),
    // RustBridgeBinary: pre-built static library for macOS (arm64, x86_64),
    // iOS (device, simulator), and Linux (arm64, x86_64). The artifactbundle
    // ships `.a` files only — SwiftPM binary targets cannot supply Swift
    // modules, so the swift-bridge generated Swift sources live in the
    // sibling RustBridge target below and link against this binary.
    .binaryTarget(
      name: "RustBridgeBinary",
      url: "https://github.com/xberg-io/xberg/releases/download/v1.0.0-rc.37/Xberg-rs.artifactbundle.zip",
      checksum: "__ALEF_SWIFT_CHECKSUM__"
    ),
    // RustBridge: Swift wrapper module owning the swift-bridge generated
    // sources. Depends on RustBridgeC for C type declarations and on
    // RustBridgeBinary so the linker picks up the static library symbols.
    .target(
      name: "RustBridge",
      dependencies: ["RustBridgeC", "RustBridgeBinary"],
      path: "packages/swift/Sources/RustBridge",
      // The pre-built static library inside RustBridgeBinary references Apple
      // system frameworks (e.g. reqwest's proxy detection pulls in the Rust
      // `system_configuration` crate → `SC*` symbols) and native system
      // libraries (e.g. the archive/`xz2` path pulls in `lzma-sys` →
      // `_lzma_stream_decoder`). The artifactbundle ships only the `.a`, so these
      // must be linked by the consumer. `liblzma` ships in the macOS SDK and on
      // Linux.
      linkerSettings: [
        .linkedLibrary("lzma"),
        // The pre-built static library pulls in C++ dependencies (onnxruntime,
        // tesseract, ClipperLib) that reference the C++ runtime/ABI
        // (`__cxa_throw`, `__gxx_personality_v0`, `__cxa_guard_acquire`, ...). A
        // `.a` archive does not carry the transitive `-lc++`/`-lstdc++`
        // system-lib dependency, so the consumer must link the C++ standard
        // library explicitly or the final link fails with undefined symbols
        // from those crates.
        .linkedLibrary("c++", .when(platforms: [.macOS, .iOS])),
        .linkedLibrary("stdc++", .when(platforms: [.linux])),
        .linkedFramework("Security", .when(platforms: [.macOS, .iOS])),
        .linkedFramework("CoreFoundation", .when(platforms: [.macOS, .iOS])),
        .linkedFramework("SystemConfiguration", .when(platforms: [.macOS])),
      ]
    ),
    .target(
      name: "Xberg",
      dependencies: ["RustBridge", "RustBridgeC"],
      path: "packages/swift/Sources/Xberg"
    ),
  ]
)
