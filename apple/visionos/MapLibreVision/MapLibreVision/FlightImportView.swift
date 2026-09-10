import SwiftUI
import UniformTypeIdentifiers

extension UTType {
    static let flightGPX = UTType(importedAs: "org.topografix.gpx", conformingTo: .xml)
    static let flightKML = UTType(importedAs: "com.google.earth.kml", conformingTo: .xml)
    static let flightRecording = UTType(exportedAs: "com.sokolysystems.flight-track", conformingTo: .json)
}

struct FlightImportView: View {
    @EnvironmentObject private var session: GlobeSession
    let preview: GlobeSession.ImportPreview
    @State private var title = ""
    @State private var elevation: FlightImport.Elevation?

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Import flight").font(.title2.bold())
            TextField("Flight name", text: $title).textFieldStyle(.roundedBorder)
            Text("\(preview.track.observations.count) positions · \(Int(preview.track.duration / 60)) min")
            Text(preview.track.source.coverage).font(.caption).foregroundStyle(.secondary)
            if preview.needsDatum {
                Picker("GPX elevations", selection: $elevation) {
                    Text("Choose elevation reference").tag(FlightImport.Elevation?.none)
                    ForEach(FlightImport.Elevation.allCases, id: \.self) { Text($0.rawValue).tag(Optional($0)) }
                }
                Text("Confirm the exporter’s elevation reference. WGS84 heights require an EGM96 geoidheight at every point. If the reference is unknown, cancel and check the source.")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text("The recording opens paused. Your current viewpoint stays in place until you choose FPV or Chase.")
                .font(.subheadline)
            if !session.status.isEmpty { Text(session.status).foregroundStyle(.orange).font(.caption) }
            HStack {
                Button("Cancel") { session.cancelImport() }
                Spacer()
                Button("Add flight") { Task { await session.finishImport(elevation: elevation ?? .msl, title: title) } }
                    .buttonStyle(.borderedProminent).disabled(session.importing || (preview.needsDatum && elevation == nil))
            }
        }.padding(26).frame(width: 440)
        .onAppear { title = preview.track.displayTitle }
    }
}

struct FlightImportPresentation: ViewModifier {
    @EnvironmentObject private var session: GlobeSession
    @Environment(\.openWindow) private var openWindow
    @Environment(\.scenePhase) private var phase
    let immersiveControls: Bool

    func body(content: Content) -> some View {
        content
            .onOpenURL { url in
                if session.immersed {
                    session.controlsExpanded = true
                    openWindow(id: GlobeSession.controlsID, value: GlobeSession.controlsID)
                }
                Task { await session.receive(url) }
            }
            .onChange(of: phase, initial: true) { _, phase in
                if phase == .active { Task { await session.receiveInbox() } }
            }
            .dropDestination(for: URL.self) { urls, _ in
                guard let url = urls.first else { return false }
                Task { await session.receive(url) }
                return true
            }
            .sheet(item: Binding(get: {
                session.immersed == immersiveControls ? session.preview : nil
            }, set: { if $0 == nil { session.cancelImport() } }), onDismiss: {
                Task { await session.receiveInbox() }
            }) { preview in
                FlightImportView(preview: preview).environmentObject(session)
            }
    }
}
