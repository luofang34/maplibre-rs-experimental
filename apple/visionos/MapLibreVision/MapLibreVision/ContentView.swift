import SwiftUI

struct ContentView: View {
    @Environment(\.openImmersiveSpace) private var openImmersiveSpace
    @Environment(\.dismissImmersiveSpace) private var dismissImmersiveSpace
    @ObservedObject private var modeStore = MapModeStore.shared
    @State private var isImmersed = false
    @State private var isOpening = false
    @State private var status = ""

    var body: some View {
        VStack(spacing: 16) {
            Text("MapLibre Vision").font(.title2.bold())
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    Picker("View", selection: Binding(
                        get: { modeStore.mode }, set: { modeStore.fly(to: $0) })) {
                        ForEach(MapMode.allCases) { mode in Text(mode.title).tag(mode) }
                    }.pickerStyle(.segmented)
                    if modeStore.isGlobe {
                        Label("Drag with one pinch to turn the globe.", systemImage: "hand.draw")
                        Text("Move two pinched hands together to place it. Spread to zoom; twist to turn.")
                            .foregroundStyle(.secondary)
                    } else {
                        VStack(alignment: .leading, spacing: 12) {
                            HStack {
                                Text("Map tilt")
                                Spacer()
                                Text("\(modeStore.tiltDegrees.formatted(.number.precision(.fractionLength(0))))°").monospacedDigit()
                            }
                            Slider(value: Binding(get: { modeStore.tiltDegrees }, set: { modeStore.setTilt($0) }), in: 0...90)
                                .accessibilityLabel("Map tilt")
                            Text("0° aligns the map with the room. Your head remains free to look around.")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                        Text("Drag to move. Spread two pinches to zoom. Move both hands sideways to orbit or vertically to tilt.")
                            .foregroundStyle(.secondary)
                    }
                    if let selection = modeStore.selectedFeature {
                        Divider()
                        VStack(alignment: .leading, spacing: 8) {
                            Text(selection.title).font(.headline)
                            if selection.coordinates.count == 2 {
                                Text(String(format: "%.4f°, %.4f°", selection.coordinates[1], selection.coordinates[0]))
                                    .font(.caption).foregroundStyle(.secondary)
                            }
                            Button("Clear selection") { modeStore.selectedFeature = nil }
                        }.accessibilityElement(children: .contain)
                    }
                    if !status.isEmpty { Text(status).foregroundStyle(.red) }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 4)
            }
            Divider()
            HStack {
                Button(modeStore.isGlobe ? "North up" : "Level map") { modeStore.resetLevel() }
                Spacer()
                Button(isImmersed ? "Leave the map" : "Enter the map") {
                    Task { await toggleMap() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(isOpening)
            }
        }
        .padding(24)
        .frame(minWidth: 360, idealWidth: 420, maxWidth: 520,
               minHeight: 360, idealHeight: 460, maxHeight: 640)
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

    private func toggleMap() async {
        isOpening = true
        defer { isOpening = false }
        status = ""
        if isImmersed {
            await dismissImmersiveSpace()
            isImmersed = false
        } else {
            let result = await openImmersiveSpace(id: MapRenderer.spaceID)
            maplibre_visionos_note("immersive space open result: \(result)")
            switch result {
            case .opened: isImmersed = true
            case .userCancelled: break
            case .error: status = "The map could not open. Please try again."
            @unknown default: status = "The map could not open. Please try again."
            }
        }
    }
}
