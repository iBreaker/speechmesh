// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "ApplePadSpeaker",
    platforms: [
        .iOS(.v16),
    ],
    products: [
        .executable(
            name: "speechmesh-pad-speaker",
            targets: ["ApplePadSpeaker"],
        ),
    ],
    targets: [
        .executableTarget(
            name: "ApplePadSpeaker",
            linkerSettings: [
                .linkedFramework("AVFoundation"),
            ],
        ),
    ],
)
