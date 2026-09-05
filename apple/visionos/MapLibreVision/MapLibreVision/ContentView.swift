import SwiftUI

/// The launcher window: shows the renderer version and opens the immersive map.
struct ContentView: View {
    @Environment(\.openImmersiveSpace) private var openImmersiveSpace
    @Environment(\.dismissImmersiveSpace) private var dismissImmersiveSpace
    @State private var isImmersed = false
    @State private var status = ""

    private var rendererVersion: String {
        String(cString: maplibre_visionos_version())
    }

    var body: some View {
        VStack(spacing: 24) {
            Text("MapLibre Vision")
                .font(.largeTitle)
            Text("Renderer \(rendererVersion)")
                .font(.caption)
                .foregroundStyle(.secondary)
            Text("Globe and worldwide terrain, drawn by maplibre-rs into the compositor.")
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
            Button(isImmersed ? "Leave the map" : "Enter the map") {
                Task {
                    if isImmersed {
                        await dismissImmersiveSpace()
                        isImmersed = false
                    } else {
                        switch await openImmersiveSpace(id: MapRenderer.spaceID) {
                        case .opened:
                            isImmersed = true
                        case .userCancelled:
                            status = "Cancelled"
                        case .error:
                            status = "The immersive space could not open"
                        @unknown default:
                            status = "Unknown result"
                        }
                    }
                }
            }
            if !status.isEmpty {
                Text(status).foregroundStyle(.red)
            }
        }
        .padding(48)
        .glassBackgroundEffect()
        .task {
            // `--enter` opens the map straight away, for scripted runs in the simulator.
            if ProcessInfo.processInfo.arguments.contains("--enter"), !isImmersed,
               case .opened = await openImmersiveSpace(id: MapRenderer.spaceID)
            {
                isImmersed = true
            }
        }
    }
}
