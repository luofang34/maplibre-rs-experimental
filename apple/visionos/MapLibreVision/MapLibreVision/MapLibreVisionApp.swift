import CompositorServices
import SwiftUI

/// Compositor settings for the map: one colour texture per eye in the renderer's format, so
/// the map's frame can be copied into the drawable without conversion, and a depth texture
/// the map writes for reprojection and for blending the globe with the room.
struct MapLayerConfiguration: CompositorLayerConfiguration {
    func makeConfiguration(
        capabilities: LayerRenderer.Capabilities,
        configuration: inout LayerRenderer.Configuration
    ) {
        configuration.colorFormat = .bgra8Unorm_srgb
        configuration.depthFormat = .depth32Float
        configuration.layout = .dedicated
        configuration.isFoveationEnabled = false
    }
}

@main
struct MapLibreVisionApp: App {
    @ObservedObject private var modeStore = MapModeStore.shared

    @Environment(\.openWindow) private var openWindow
    @StateObject private var session = GlobeSession()

    var body: some Scene {
        WindowGroup(id: GlobeSession.controlsID, for: String.self) { _ in
            ContentView().environmentObject(session)
        } defaultValue: { GlobeSession.controlsID }
        .handlesExternalEvents(matching: ["*"])
        .defaultSize(width: 440, height: 620)
        .windowResizability(.contentSize)
        .defaultLaunchBehavior(.presented)

        ImmersiveSpace(id: MapRenderer.spaceID) {
            ImmersiveMapView(session: session)
            .onAppear {
                session.immersed = true
            }
            .onDisappear {
                openWindow(id: GlobeSession.controlsID, value: GlobeSession.controlsID)
                session.immersed = false
                session.replay.pause()
                session.save()
            }
        }
        .immersionStyle(selection: $modeStore.immersionStyle, in: .mixed, .full)
    }
}

private struct ImmersiveMapView: CompositorContent {
    @ObservedObject var session: GlobeSession
    @Environment(\.openWindow) private var openWindow
    var body: some CompositorContent {
        CompositorLayer(configuration: MapLayerConfiguration()) { layer in
            MapRenderer(layerRenderer: layer, replay: session.replay) {
                Task { @MainActor in session.controlsRequest = session.controlsRequest &+ 1 }
            }.start()
        }
        .onChange(of: session.controlsRequest) { _, _ in
            openWindow(id: GlobeSession.controlsID, value: GlobeSession.controlsID)
        }
    }
}
