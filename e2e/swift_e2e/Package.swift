// swift-tools-version: 6.0
import PackageDescription

let package = Package(
  name: "E2eSwift",
  platforms: [
    .macOS(.v13),
    .iOS(.v16),
  ],
  dependencies: [
    .package(path: "../../packages/swift"),
  ],
  targets: [
    .testTarget(
      name: "XbergE2ETests",
      dependencies: [.product(name: "Xberg", package: "swift")]
    ),
  ]
)
