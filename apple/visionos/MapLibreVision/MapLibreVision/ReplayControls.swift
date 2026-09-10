import SwiftUI

struct ReplayControls: View {
    @EnvironmentObject private var session: GlobeSession
    @State private var resumeAfterScrub = false
    @State private var showSource = false
    @State private var importing = false
    @State private var showReference = false

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.2)) { _ in
            let frame = session.replay.frame()
            VStack(alignment: .leading, spacing: 10) {
                HStack {
                    Menu {
                        ForEach(session.tracks) { entry in
                            Button(entry.title + (entry.simulated ? " · Simulation" : "")) {
                                Task { await session.select(entry) }
                            }
                        }
                        Divider()
                        Button("Restore demo flights") { Task { await session.restoreDemos() } }
                        Button("Import track…", systemImage: "square.and.arrow.down") { importing = true }
                        if session.tracks.contains(where: { $0.id == session.selectedTrackID }) {
                            Button("Remove flight", role: .destructive) { Task { await session.removeSelectedTrack() } }
                        }
                    } label: {
                        Label(frame.track?.displayTitle ?? "Flight library", systemImage: "airplane")
                            .font(.headline).lineLimit(2)
                    }.buttonStyle(.plain)
                    Spacer()
                    Button { showSource.toggle() } label: { Image(systemName: "info.circle") }
                        .buttonStyle(.plain).accessibilityLabel("Recording source")
                }
                if let track = frame.track {
                    Text(track.isSimulation ? "SIMULATED FLIGHT · \(track.aircraft)" : "\(track.callsign) · \(track.aircraft)")
                        .font(.caption).foregroundStyle(track.isSimulation ? .orange : .secondary)
                    HStack {
                        Button { session.replay.toggle(); session.save() } label: {
                            Image(systemName: frame.playing ? "pause.fill" : "play.fill")
                        }.accessibilityLabel(frame.playing ? "Pause flight" : "Play flight")
                        Slider(value: Binding(get: { frame.elapsed }, set: { session.replay.seek($0) }),
                               in: 0...max(track.duration, 1), onEditingChanged: scrub)
                            .accessibilityLabel("Flight playback position")
                        Picker("Speed", selection: Binding(get: { frame.rate }, set: { session.replay.setRate($0); session.save() })) {
                            Text("1×").tag(1.0); Text("4×").tag(4.0); Text("16×").tag(16.0)
                        }.labelsHidden().frame(width: 84)
                    }
                    HStack {
                        Text("\(Int(frame.elapsed) / 60):\(String(format: "%02d", Int(frame.elapsed) % 60)) / \(Int(track.duration) / 60):\(String(format: "%02d", Int(track.duration) % 60))")
                        Spacer()
                        if let point = frame.observation {
                            Text("\(Int(point.altitudeMSL / 0.3048)) ft MSL · " + (point.hasVelocity ? "\(Int(point.groundSpeed * 3600 / 1852)) kt GS" : "GS MISSING"))
                        } else { Text("TRACK DATA GAP").foregroundStyle(.orange) }
                    }.font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                    if let seconds = frame.returnSeconds {
                        HStack {
                            Text("Returning to ownship in \(seconds)s").monospacedDigit()
                            Spacer()
                            Button("Stay free") { session.replay.setView(.free) }
                        }.font(.caption)
                    }
                    if frame.elapsed >= track.duration { Text("End of available track").font(.caption) }
                    if session.immersed {
                        Picker("Camera", selection: Binding(get: { frame.view }, set: { session.replay.setView($0) })) {
                            ForEach(FlightView.allCases, id: \.self) { Text($0.rawValue).tag($0) }
                        }.pickerStyle(.segmented)
                        HStack {
                            Text(frame.following ? "Look around with your head. Drag to detach and explore." : "Free camera · Return to FPV to rejoin the flight.")
                                .font(.caption).foregroundStyle(.secondary)
                            if frame.following {
                                Button("Look forward") { session.replay.recenter() }.font(.caption)
                            }
                        }
                    }
                } else { Text(session.replay.loadingError ?? "No recording available").foregroundStyle(.red) }
                HStack {
                    Button("Import track", systemImage: "square.and.arrow.down") { importing = true }
                    Spacer()
                    Button("SVS reference") { showReference = true }
                }.font(.caption).buttonStyle(.plain)
            }
        }
        .fileImporter(isPresented: $importing, allowedContentTypes: [.flightGPX, .flightKML, .flightRecording, .json]) { result in
            switch result {
            case .success(let url): Task { await session.receive(url) }
            case .failure(let error): session.status = error.localizedDescription
            }
        }
        .popover(isPresented: $showSource) { source }
        .sheet(isPresented: $showReference) { SVSReferenceView() }
    }

    private var source: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let track = session.replay.track {
                Text(track.isSimulation ? "Simulated scenario" : "Recorded flight").font(.headline)
                Text(track.source.coverage)
                Text(track.source.altitudeConversion)
                Text(track.source.license).font(.caption)
                if let url = URL(string: track.source.url), ["https", "http"].contains(url.scheme ?? "") {
                    Link("Track source", destination: url)
                }
            }
            Text("IAS, heading and attitude are shown only when provided. GPS track is not aircraft heading.")
            Text("Terrain replay · SIM / NOT FOR FLIGHT").font(.caption).foregroundStyle(.orange)
            Text("Globe: Natural Earth, public domain · rendered by MapLibre").font(.caption)
        }.padding(24).frame(width: 400)
    }

    private func scrub(_ editing: Bool) {
        if editing {
            resumeAfterScrub = session.replay.frame().playing
            session.replay.pause()
        } else {
            if resumeAfterScrub { session.replay.toggle() }
            session.save()
        }
    }
}
