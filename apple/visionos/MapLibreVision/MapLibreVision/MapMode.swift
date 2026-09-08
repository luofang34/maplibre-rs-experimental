import CompositorServices
import Foundation
import SwiftUI

/// The two viewpoints the launcher's buttons fly to: the globe on the table, or the world
/// around the viewer. Between them the height is continuous, so the buttons are shortcuts
/// and a zoom reaches either end without them.
enum MapMode: String, CaseIterable, Identifiable {
    case tableGlobe
    case immersive

    var id: String { rawValue }

    var title: String {
        switch self {
        case .tableGlobe: "Table globe"
        case .immersive: "Immersive"
        }
    }

    /// The height the button flies to.
    var height: Double {
        switch self {
        case .tableGlobe: MapPlacement.tableHeight
        case .immersive: MapPlacement.flyToHeight
        }
    }
}

extension MapImmersion {
    /// A globe in the room needs passthrough around it; the world around the viewer replaces the room.
    var style: any ImmersionStyle {
        switch self {
        case .mixed: .mixed
        case .full: .full
        }
    }
}

/// The launcher's requests and the room's immersion, shared with the render thread.
final class MapModeStore: ObservableObject {
    static let shared = MapModeStore()

    /// The mode last flown to, for the launcher's buttons.
    @Published var mode: MapMode
    @Published var immersionStyle: any ImmersionStyle
    @Published var tiltDegrees = 0.0
    @Published var isGlobe = true
    @Published var selectedFeature: MapSelection?

    private let lock = NSLock()
    private var requested: MapMode?
    private var requestedTilt: Double?
    private var requestedLevel = false
    private var reportedGlobe: Bool?
    private var reportedTilt: Double?
    /// `--height N` starts the viewer N metres above the focus, for scripted runs.
    let initialHeight: Double

    private init() {
        let initial = MapModeStore.modeFromArguments()
        mode = initial
        let height = MapModeStore.heightFromArguments() ?? initial.height
        initialHeight = height
        immersionStyle = MapPlacement.immersion(forHeight: height).style
    }

    /// Flies to a mode's viewpoint.
    func fly(to mode: MapMode) {
        self.mode = mode
        maplibre_visionos_note("flight requested to \(mode.rawValue)")
        lock.withLock { requested = mode }
    }

    func setTilt(_ degrees: Double) {
        tiltDegrees = degrees
        lock.withLock { requestedTilt = degrees * .pi / 180 }
    }

    func resetLevel() {
        tiltDegrees = 0
        lock.withLock { requestedLevel = true }
    }

    func takeControls(isGlobe: Bool, tilt: Double) -> (Double?, Bool) {
        lock.withLock {
            let degrees = (tilt * 180 / .pi * 10).rounded() / 10
            if reportedGlobe != isGlobe || reportedTilt != degrees, requestedTilt == nil {
                reportedGlobe = isGlobe
                reportedTilt = degrees
                Task { @MainActor in
                    self.isGlobe = isGlobe
                    self.tiltDegrees = degrees
                }
            }
            defer { requestedTilt = nil; requestedLevel = false }
            return (requestedTilt, requestedLevel)
        }
    }

    func showSelection(_ selection: MapSelection?) {
        Task { @MainActor in self.selectedFeature = selection }
    }

    /// The mode a button asked for since the last call, for the render thread.
    func takeFlightRequest() -> MapMode? {
        lock.withLock {
            let request = requested
            requested = nil
            return request
        }
    }

    /// Shows an immersion. The renderer calls this as the viewpoint crosses the bridge
    /// between the ground and the table, so the room does not vanish around a globe that
    /// is still small.
    func showImmersion(_ immersion: MapImmersion) {
        maplibre_visionos_note("immersion changing to \(immersion)")
        Task { @MainActor in
            self.immersionStyle = immersion.style
        }
    }

    /// `--mode tableGlobe` or `--mode immersive` picks the starting viewpoint for scripted runs.
    private static func modeFromArguments() -> MapMode {
        let arguments = ProcessInfo.processInfo.arguments
        if let index = arguments.firstIndex(of: "--mode"), index + 1 < arguments.count,
           let mode = MapMode(rawValue: arguments[index + 1])
        {
            return mode
        }
        return .tableGlobe
    }

    private static func heightFromArguments() -> Double? {
        let arguments = ProcessInfo.processInfo.arguments
        guard let index = arguments.firstIndex(of: "--height"), index + 1 < arguments.count,
              let height = Double(arguments[index + 1]), height > 0
        else {
            return nil
        }
        return height
    }
}
