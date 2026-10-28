// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "Snowdrift",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(
            name: "Snowdrift",
            path: "Sources/Snowdrift"
        ),
    ]
)
