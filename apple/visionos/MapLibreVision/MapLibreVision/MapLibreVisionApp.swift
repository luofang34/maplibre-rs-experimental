import CompositorServices
import SwiftUI

/// Compositor settings for the map: one colour texture per eye in the renderer's format, so
/// the map's frame can be copied into the drawable without conversion.
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
    @State private var immersionStyle: ImmersionStyle = .full

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
        .immersionStyle(selection: $immersionStyle, in: .full)
    }
}
