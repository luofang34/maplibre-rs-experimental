// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "MapInteraction",
    platforms: [.macOS(.v13)],
    targets: [
        .target(name: "FlightExchange", path: "Shared"),
        .target(
            name: "MapInteraction", dependencies: ["FlightExchange"], path: "MapLibreVision",
            exclude: ["ContentView.swift", "Info.plist", "MapLibreVision.entitlements", "MapEyeTargets.swift", "MapGestures.swift",
                      "MapLibreVisionApp.swift", "MapMode.swift", "MapSelection.swift", "MapRenderer.swift", "style.json", "Resources", "FlightTrack.metal", "DeskGlobeView.swift", "ReplayControls.swift", "FlightLibraryView.swift", "TrackOverlayRenderer.swift", "FlightImportView.swift", "SVSReferenceView.swift", "FlightHUDRenderer.swift", "FlightHUDSurface.swift", "FlightSymbolRenderer.swift", "FlightBridge.h"],
            sources: ["MapPlacement.swift", "MapPose.swift", "MapOrbit.swift", "MapNavigation.swift", "MapGestureInput.swift", "GlobeDrag.swift", "MapDragPlane.swift", "MapGestureRecognizer.swift", "MapMemoryBudget.swift", "MapZoomStress.swift", "FlightTrack.swift", "FlightReplay.swift", "FlightCamera.swift", "GlobeGeometry.swift", "DeskGlobeState.swift", "FlightImport.swift", "FlightXML.swift", "FlightLibrary.swift", "FlightRoute.swift", "FlightHUDGeometry.swift", "GlobeSession.swift"]),
        .testTarget(name: "MapInteractionTests", dependencies: ["MapInteraction", "FlightExchange"], path: "Tests",
                    exclude: ["check-frame-recovery.py"])
    ])
