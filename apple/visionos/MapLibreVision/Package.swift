// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "MapInteraction",
    platforms: [.macOS(.v13)],
    targets: [
        .target(
            name: "MapInteraction", path: "MapLibreVision",
            exclude: ["ContentView.swift", "Info.plist", "MapEyeTargets.swift", "MapGestures.swift",
                      "MapLibreVisionApp.swift", "MapMode.swift", "MapSelection.swift", "MapRenderer.swift", "style.json"],
            sources: ["MapPlacement.swift", "MapPose.swift", "MapOrbit.swift", "MapNavigation.swift", "MapGestureInput.swift", "GlobeDrag.swift", "MapDragPlane.swift", "MapGestureRecognizer.swift", "MapMemoryBudget.swift", "MapZoomStress.swift"]),
        .testTarget(name: "MapInteractionTests", dependencies: ["MapInteraction"], path: "Tests",
                    exclude: ["check-frame-recovery.py"])
    ])
