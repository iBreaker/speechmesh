// swift-tools-version: 6.1
import PackageDescription

let package = Package(
    name: "AppleASRBridge",
    platforms: [
        .macOS(.v15),
    ],
    products: [
        .executable(
            name: "apple-asr-bridge",
            targets: ["AppleASRBridge"]
        ),
    ],
    targets: [
        .executableTarget(
            name: "AppleASRBridge",
            linkerSettings: [
                .linkedFramework("AVFoundation"),
                .linkedFramework("Speech"),
            ]
        ),
    ]
)
