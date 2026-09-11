import SwiftUI

struct ContentView: View {
    @Environment(\.openImmersiveSpace) private var openImmersiveSpace
    @Environment(\.dismissImmersiveSpace) private var dismissImmersiveSpace
    @ObservedObject private var modeStore = MapModeStore.shared
    @EnvironmentObject private var session: GlobeSession
    @Environment(\.scenePhase) private var scenePhase
    @State private var isOpening = false
    @State private var status = ""

    var body: some View {
        VStack(spacing: 16) {
            HStack {
                Text("Map controls").font(.headline)
                Spacer()
                Button("Fly track", systemImage: "airplane") { Task { await enterMap(follow: true) } }.disabled(session.replay.track == nil)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    ReplayControls()
                    Divider()
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
                            Slider(value: Binding(get: { modeStore.tiltDegrees }, set: { modeStore.setTilt($0) }),
                                   in: 0...90, onEditingChanged: { modeStore.editTilt($0) })
                                .accessibilityLabel("Map tilt")
                            Text("0° aligns the map with the room. Your head remains free to look around.")
                                .font(.caption).foregroundStyle(.secondary)
                            if modeStore.tiltLimited {
                                Text("Tilt limited to stay above terrain.").font(.caption)
                            }
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
                    if !session.status.isEmpty { Text(session.status).font(.caption).foregroundStyle(.orange) }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 4)
            }
            Divider()
            HStack {
                Button(modeStore.isGlobe ? "North up" : "Level map") { modeStore.resetLevel() }
                Spacer()
                Button(session.immersed ? "Leave map" : "Explore map") {
                    Task { await toggleMap() }
                }
                .buttonStyle(.borderedProminent)
                .disabled(isOpening)
            }
        }
        .padding(20)
        .frame(minWidth: 360, idealWidth: 420, maxWidth: 520,
               minHeight: 360, idealHeight: 620, maxHeight: 800)
        .modifier(FlightImportPresentation())
        .onChange(of: scenePhase) { _, phase in
            if phase != .active { session.save() }
        }
        .task {
            await session.loadLibrary()
            guard !session.didRunLaunchActions else { return }
            session.didRunLaunchActions = true
            let launch = ProcessInfo.processInfo.arguments
            if let index = launch.firstIndex(of: "--import-track"), index + 1 < launch.count {
                await session.receive(URL(fileURLWithPath: launch[index + 1]))
            }
            if launch.contains("--replay") { await enterMap(follow: true) }
            else if launch.contains("--mode") { await enterMap(follow: false) }
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
        if session.immersed {
            await dismissImmersiveSpace()
            session.immersed = false
            session.replay.pause()
            session.save()
        } else { await enterMap(follow: false) }
    }

    private func enterMap(follow: Bool) async {
        guard !isOpening else { return }
        isOpening = true
        defer { isOpening = false }
        session.replay.follow(follow)
        if !session.immersed {
            let result = await openImmersiveSpace(id: MapRenderer.spaceID)
            switch result {
            case .opened: session.immersed = true
            case .userCancelled: session.replay.follow(false)
            case .error: status = "The map could not open. Please try again."
            @unknown default: status = "The map could not open. Please try again."
            }
        }
        if session.immersed && follow && !session.replay.frame().playing { session.replay.toggle() }
    }
}
