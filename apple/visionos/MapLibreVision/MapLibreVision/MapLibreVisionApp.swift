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

    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        .windowStyle(.plain)

        ImmersiveSpace(id: MapRenderer.spaceID) {
            CompositorLayer(configuration: MapLayerConfiguration()) { layerRenderer in
                MapRenderer(layerRenderer: layerRenderer).start()
            }
        }
        // The table globe sits in the room; the immersive view replaces it. Changing the
        // mode moves the scene and the immersion together.
        .immersionStyle(selection: $modeStore.immersionStyle, in: .mixed, .full)
    }
}
