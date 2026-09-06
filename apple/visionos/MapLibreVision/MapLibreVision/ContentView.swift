import SwiftUI

/// The launcher window: shows the renderer version, picks the mode and opens the map.
struct ContentView: View {
    @Environment(\.openImmersiveSpace) private var openImmersiveSpace
    @Environment(\.dismissImmersiveSpace) private var dismissImmersiveSpace
    @ObservedObject private var modeStore = MapModeStore.shared
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
            HStack(spacing: 16) {
                ForEach(MapMode.allCases) { mode in
                    Button(mode.title) {
                        modeStore.fly(to: mode)
                    }
                    .buttonStyle(.bordered)
                    .tint(modeStore.mode == mode ? .accentColor : .secondary)
                }
            }
            Text("Pinch and drag to turn the globe or move the world. Pinch with both hands and pull them apart to zoom in, all the way from the globe to the street and back; turn them to rotate. The buttons only fly you there.")
                .multilineTextAlignment(.center)
                .frame(maxWidth: 420)
                .font(.caption)
                .foregroundStyle(.secondary)
            Button(isImmersed ? "Leave the map" : "Enter the map") {
                Task {
                    if isImmersed {
                        await dismissImmersiveSpace()
                        maplibre_visionos_note("immersive space dismissed by the launcher")
                        isImmersed = false
                    } else {
                        let result = await openImmersiveSpace(id: MapRenderer.spaceID)
                        maplibre_visionos_note("immersive space open result: \(result)")
                        switch result {
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
            // `--switch-after N` flips the mode N seconds later, so a scripted run can show
            // the move between the two placements without a hand on the picker.
            let arguments = ProcessInfo.processInfo.arguments
            if let index = arguments.firstIndex(of: "--switch-after"), index + 1 < arguments.count,
               let seconds = Double(arguments[index + 1])
            {
                try? await Task.sleep(for: .seconds(seconds))
                modeStore.fly(to: modeStore.mode == .tableGlobe ? .immersive : .tableGlobe)
            }
        }
    }
}
