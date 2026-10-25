// swift-tools-version:6.0
import PackageDescription

let package = Package(
    name: "AnyDisplay",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "AnyDisplayProtocol", targets: ["AnyDisplayProtocol"]),
        .executable(name: "anydisplay", targets: ["anydisplay"]),
    ],
    targets: [
        .target(name: "AnyDisplayProtocol"),
        // Declarations for CoreGraphics' CGVirtualDisplay classes, which ship without public headers.
        .target(name: "CVirtualDisplay"),
        .target(
            name: "AnyDisplayHost",
            dependencies: ["AnyDisplayProtocol", "CVirtualDisplay"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "anydisplay",
            dependencies: ["AnyDisplayHost", "AnyDisplayProtocol"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .testTarget(name: "AnyDisplayProtocolTests", dependencies: ["AnyDisplayProtocol"]),
    ]
)
