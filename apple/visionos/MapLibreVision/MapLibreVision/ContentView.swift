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
            if modeStore.isGlobe {
                Text("Drag with one pinch to turn. Move two pinched hands together to place the globe; spread to zoom or twist to rotate.")
                    .font(.caption).foregroundStyle(.secondary)
                    .multilineTextAlignment(.center).frame(maxWidth: 420)
            } else {
                VStack {
                    Text("View tilt: \(Int(modeStore.tiltDegrees))°")
                    Slider(value: Binding(get: { modeStore.tiltDegrees }, set: { modeStore.setTilt($0) }), in: 0...70)
                        .accessibilityLabel("View tilt")
                    Text("Drag to move. Spread two pinches to zoom. Move both pinched hands sideways to orbit or vertically to tilt; twist also turns. Look around naturally.")
                        .font(.caption).foregroundStyle(.secondary).multilineTextAlignment(.center)
                }.frame(maxWidth: 420)
            }
            if let selection = modeStore.selectedFeature {
                VStack(alignment: .leading, spacing: 4) {
                    Text(selection.title).font(.headline)
                    if selection.coordinates.count == 2 {
                        Text(String(format: "%.4f°, %.4f°", selection.coordinates[1], selection.coordinates[0]))
                            .font(.caption).foregroundStyle(.secondary)
                    }
                }
                .accessibilityElement(children: .combine)
                Button("Clear selection") { modeStore.selectedFeature = nil }
            }
            Button(modeStore.isGlobe ? "North up" : "Level view") { modeStore.resetLevel() }
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
