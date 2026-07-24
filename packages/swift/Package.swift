// swift-tools-version: 6.0
import PackageDescription
import Foundation

// NOTE: Run `cargo build -p xberg-swift` and then rerun `alef generate`
// before `swift build`. Alef materializes the swift-bridge Swift/C outputs into
// Sources/RustBridge and Sources/RustBridgeC when the Cargo build output exists.
// See README.md for the full workflow.

// Absolute path to the Cargo target dir, resolved from this manifest's own location so the
// runtime rpath is independent of the process working directory (`swift test` may chdir into
// fixture dirs). `#filePath` is a compile-time literal, so this performs no filesystem access.
let rustTargetDir = (#filePath as NSString).deletingLastPathComponent.appending("/../../target")

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
    // RustBridgeC: pure C/headers target. Swift files in RustBridge import this
    // to access C types (RustStr, etc.) produced by swift-bridge.
    // publicHeadersPath: "." exposes RustBridgeC.h to dependents.
    .target(
      name: "RustBridgeC",
      path: "Sources/RustBridgeC",
      publicHeadersPath: "."
    ),
    // RustBridge: Swift wrapper around the Rust static library.
    // Depends on RustBridgeC so the generated Swift files can use the C types.
    // linkerSettings wire the Rust staticlibs (libxberg_swift.a and libxberg_ffi.a)
    // produced by `cargo build -p xberg-swift` and the FFI crate so
    // `swift build` / `swift test` can resolve the `__swift_bridge__$*` and FFI C symbols.
    // Both target/release and target/debug are searched so either cargo profile works.
    // The FFI library is needed because the generated Swift service API code (App.swift)
    // calls FFI functions directly via @_silgen_name declarations.
    .target(
      name: "RustBridge",
      dependencies: ["RustBridgeC"],
      path: "Sources/RustBridge",
      linkerSettings: [
        .unsafeFlags([
          "-L\(rustTargetDir)/release",
          "-L\(rustTargetDir)/debug",
          // Runtime search paths: the FFI dylib's install_name is @rpath/lib...dylib, so the
          // consumer (and any test bundle linking this target) needs an LC_RPATH to dlopen it.
          // swiftc rejects `-Wl,-rpath,<p>`; the driver-native spelling is `-Xlinker -rpath -Xlinker <p>`.
          "-Xlinker", "-rpath", "-Xlinker", "\(rustTargetDir)/release",
          "-Xlinker", "-rpath", "-Xlinker", "\(rustTargetDir)/debug",
        ]),
        .linkedLibrary("xberg_swift"),
        .linkedLibrary("xberg_ffi"),
        // The Rust staticlib records native-library dependencies (e.g. `lzma-sys`
        // via the archive/`xz2` path emits `cargo:rustc-link-lib`) that cargo would
        // resolve when it drives the final link, but a `staticlib` `.a` does not
        // embed them and SwiftPM does not read cargo's link metadata, so undefined
        // symbols like `_lzma_stream_decoder` surface at the swift link step. Link
        // the system library here. `liblzma` ships in the macOS SDK and on Linux.
        .linkedLibrary("lzma"),
        // The Rust staticlib pulls in C++ dependencies (onnxruntime, tesseract,
        // ClipperLib) that reference the C++ runtime/ABI (`__cxa_throw`,
        // `__gxx_personality_v0`, `__cxa_guard_acquire`, ...). A `staticlib` `.a`
        // does not carry the transitive `-lc++`/`-lstdc++` system-lib dependency,
        // so SwiftPM must link the C++ standard library explicitly or the final
        // link fails with undefined symbols from those crates.
        .linkedLibrary("c++", .when(platforms: [.macOS, .iOS])),
        .linkedLibrary("stdc++", .when(platforms: [.linux])),
        .linkedFramework("Security", .when(platforms: [.macOS, .iOS])),
        .linkedFramework("CoreFoundation", .when(platforms: [.macOS, .iOS])),
        .linkedFramework("SystemConfiguration", .when(platforms: [.macOS])),
      ]
    ),
    .target(
      name: "Xberg", dependencies: ["RustBridge"],
      path: "Sources/Xberg",
      exclude: ["LICENSE"]),
    .testTarget(
      name: "XbergTests", dependencies: ["Xberg"],
      path: "Tests/XbergTests"),
  ]
)
