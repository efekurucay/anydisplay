// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "UseThatScreen",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "UseThatScreenProtocol", targets: ["UseThatScreenProtocol"]),
        .executable(name: "usethatscreen", targets: ["usethatscreen"]),
    ],
    targets: [
        .target(name: "UseThatScreenProtocol"),
        // Declarations for CoreGraphics' CGVirtualDisplay classes, which ship without public headers.
        .target(name: "CVirtualDisplay"),
        .target(
            name: "UseThatScreenHost",
            dependencies: ["UseThatScreenProtocol", "CVirtualDisplay"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "usethatscreen",
            dependencies: ["UseThatScreenHost", "UseThatScreenProtocol"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(name: "UseThatScreenProtocolTests", dependencies: ["UseThatScreenProtocol"]),
        .testTarget(name: "UseThatScreenHostTests", dependencies: ["UseThatScreenHost"]),
    ]
)
