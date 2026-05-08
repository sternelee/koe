// swift-tools-version: 5.9

import PackageDescription

let package = Package(
    name: "KoeAppleTranslation",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "KoeAppleTranslation", type: .static, targets: ["KoeAppleTranslation"]),
    ],
    targets: [
        .target(name: "KoeAppleTranslation"),
    ]
)
