// swift-tools-version: 6.1
import PackageDescription

let package = Package(
    name: "SpeechMeshSwift",
    platforms: [
        .iOS(.v17),
        .macOS(.v14),
    ],
    products: [
        .library(
            name: "SpeechMeshSwift",
            targets: ["SpeechMeshSwift"]
        ),
    ],
    targets: [
        .target(
            name: "SpeechMeshSwift"
        ),
        .testTarget(
            name: "SpeechMeshSwiftTests",
            dependencies: ["SpeechMeshSwift"]
        ),
    ]
)
