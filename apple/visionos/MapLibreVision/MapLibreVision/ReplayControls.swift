import SwiftUI

struct ReplayControls: View {
    @EnvironmentObject private var session: GlobeSession
    @State private var resumeAfterScrub = false
    @State private var showSource = false

    var body: some View {
        TimelineView(.periodic(from: .now, by: 0.25)) { _ in
            let frame = session.replay.frame()
            VStack(alignment: .leading, spacing: 10) {
                if let track = session.replay.track {
                    HStack {
                        Label("Innsbruck approach", systemImage: "airplane.arrival").font(.headline)
                        Spacer()
                        Button { showSource.toggle() } label: { Image(systemName: "info.circle") }
                            .buttonStyle(.plain).accessibilityLabel("Recording source")
                    }
                    Text("\(track.callsign) · \(track.registration) · \(track.aircraft)")
                        .font(.caption).foregroundStyle(.secondary)
                    HStack {
                        Button { session.replay.toggle(); session.save() } label: {
                            Image(systemName: frame.playing ? "pause.fill" : "play.fill")
                        }.accessibilityLabel(frame.playing ? "Pause approach" : "Play approach")
                        Slider(value: Binding(get: { frame.elapsed }, set: { session.replay.seek($0) }),
                               in: 0...max(track.duration, 1), onEditingChanged: scrub)
                            .accessibilityLabel("Approach playback position")
                        Picker("Speed", selection: Binding(get: { frame.rate }, set: { session.replay.setRate($0); session.save() })) {
                            Text("1×").tag(1.0); Text("4×").tag(4.0); Text("16×").tag(16.0)
                        }.labelsHidden().frame(width: 84)
                    }
                    HStack {
                        Text(Date(timeIntervalSince1970: track.startUTC + frame.elapsed).formatted(
                            Date.FormatStyle(date: .omitted, time: .standard, timeZone: .gmt)) + " UTC")
                        Spacer()
                        if let point = frame.observation {
                            Text("\(Int(point.altitudeMSL / 0.3048)) ft MSL · \(Int(point.groundSpeed * 3600 / 1852)) kt")
                        } else { Text("No receiver coverage") }
                    }.font(.caption.monospacedDigit()).foregroundStyle(.secondary)
                    if frame.elapsed >= track.duration {
                        Text("End of recording · coverage ends before the runway").font(.caption)
                    }
                    if session.immersed {
                        Toggle("Follow aircraft", isOn: Binding(get: { frame.following }, set: { session.replay.follow($0) }))
                            .font(.subheadline)
                    }
                } else { Text(session.replay.loadingError ?? "No recording available").foregroundStyle(.red) }
            }
        }
        .popover(isPresented: $showSource) {
            if let track = session.replay.track {
                VStack(alignment: .leading, spacing: 14) {
                    Text("Recorded ADS-B · 8 Sep 2026").font(.headline)
                    Text(track.source.coverage)
                    Text("GNSS altitude is converted from WGS84 to EGM96 mean sea level. The aircraft symbol follows ground track, not recorded attitude.")
                    Link("ADSB.lol recording · ODbL 1.0", destination: URL(string: track.source.url) ?? URL(fileURLWithPath: "/"))
                    Link("Database license", destination: URL(string: "https://opendatacommons.org/licenses/odbl/1-0/")!)
                    Text("Globe: Natural Earth, public domain · rendered by MapLibre").font(.caption)
                }.padding(24).frame(width: 400)
            }
        }
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
