// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "MapInteraction",
    platforms: [.macOS(.v13)],
    targets: [
        .target(
            name: "MapInteraction", path: "MapLibreVision",
            exclude: ["ContentView.swift", "Info.plist", "MapEyeTargets.swift", "MapGestures.swift",
                      "MapLibreVisionApp.swift", "MapMode.swift", "MapSelection.swift", "MapRenderer.swift", "style.json", "Resources", "FlightTrack.metal", "DeskGlobeView.swift", "GlobeSession.swift", "ReplayControls.swift", "TrackOverlayRenderer.swift", "FlightImportView.swift", "SVSReferenceView.swift", "FlightHUDRenderer.swift", "FlightBridge.h"],
            sources: ["MapPlacement.swift", "MapPose.swift", "MapOrbit.swift", "MapNavigation.swift", "MapGestureInput.swift", "GlobeDrag.swift", "MapDragPlane.swift", "MapGestureRecognizer.swift", "MapMemoryBudget.swift", "MapZoomStress.swift", "FlightTrack.swift", "FlightReplay.swift", "FlightCamera.swift", "GlobeGeometry.swift", "DeskGlobeState.swift", "FlightImport.swift", "FlightXML.swift", "FlightLibrary.swift", "FlightRoute.swift"]),
        .testTarget(name: "MapInteractionTests", dependencies: ["MapInteraction"], path: "Tests",
                    exclude: ["check-frame-recovery.py"])
    ])
