import SwiftUI
import simd

@MainActor
final class GlobeSession: ObservableObject {
    static let homeID = "desk-globe"
    static let controlsID = "map-controls"
    private static let storageKey = "desk-globe-state"

    struct ImportPreview: Identifiable {
        let id = UUID()
        let data: Data
        let name: String
        let track: FlightTrack
        var needsDatum: Bool { name.lowercased().hasSuffix(".gpx") }
    }
    @Published var tracks: [FlightLibrary.Entry] = []
    @Published var selectedTrackID = "innsbruck-approach"
    @Published var preview: ImportPreview?
    @Published var importing = false
    private let library = FlightLibrary()
    private var loadedLibrary = false
    private var selectionRevision: UInt64 = 0

    @Published var controlsExpanded = false
    @Published var controlsRequest: UInt64 = 0
    @Published var immersed = false
    @Published var opening = false
    @Published var status = ""
    var desk: DeskGlobeState
    let replay: FlightReplay

    init() {
        let restored = UserDefaults.standard.data(forKey: Self.storageKey)
            .flatMap { try? JSONDecoder().decode(DeskGlobeState.self, from: $0) } ?? .init()
        desk = restored
        replay = FlightReplay(restored: restored)
    }

    func loadLibrary() async {
        guard !loadedLibrary else { return }
        loadedLibrary = true
        do {
            tracks = try await library.entries()
            let id = UserDefaults.standard.string(forKey: "selected-flight") ?? "innsbruck-approach"
            if let entry = tracks.first(where: { $0.id == id }), id != "innsbruck-approach" {
                let time = desk.playbackTime, rate = desk.playbackRate
                await select(entry)
                replay.seek(time)
                replay.setRate(rate)
            }
            #if DEBUG
            if let index = ProcessInfo.processInfo.arguments.firstIndex(of: "--track"),
               index + 1 < ProcessInfo.processInfo.arguments.count,
               let entry = tracks.first(where: { $0.id == ProcessInfo.processInfo.arguments[index + 1] }) {
                await select(entry)
            }
            if let index = ProcessInfo.processInfo.arguments.firstIndex(of: "--replay-rate"),
               index + 1 < ProcessInfo.processInfo.arguments.count,
               let rate = Double(ProcessInfo.processInfo.arguments[index + 1]) { replay.setRate(rate) }
            #endif
        } catch { status = "Could not load recordings: \(error.localizedDescription)" }
    }

    func select(_ entry: FlightLibrary.Entry) async {
        selectionRevision = selectionRevision &+ 1
        let request = selectionRevision
        do {
            let track = try await library.load(entry)
            guard request == selectionRevision else { return }
            replay.replace(with: track)
            selectedTrackID = entry.id
            save()
        } catch { status = error.localizedDescription }
    }

    func receive(_ url: URL) async {
        guard !importing, preview == nil else { status = "Finish the current import before opening another recording."; return }
        importing = true
        defer { importing = false }
        do {
            let name = url.lastPathComponent
            let result = try await Task.detached(priority: .userInitiated) {
                let data = try FlightImport.read(url)
                return (data, try FlightImport.decode(data, name: name))
            }.value
            preview = ImportPreview(data: result.0, name: name, track: result.1)
        } catch { status = "Could not import this track: \(error.localizedDescription)" }
    }

    func finishImport(elevation: FlightImport.Elevation) async {
        guard let preview, !importing else { return }
        importing = true
        defer { importing = false }
        do {
            let (entry, track) = try await library.importTrack(preview.data, name: preview.name, elevation: elevation)
            if !tracks.contains(entry) { tracks.append(entry) }
            selectionRevision = selectionRevision &+ 1
            replay.replace(with: track)
            selectedTrackID = entry.id
            self.preview = nil
            status = "Imported \(entry.title). Press Fly track when ready."
            save()
        } catch { status = error.localizedDescription }
    }

    func removeSelectedTrack() async {
        guard let entry = tracks.first(where: { $0.id == selectedTrackID && $0.imported }),
              let fallback = tracks.first(where: { !$0.imported }) else { return }
        do {
            try await library.remove(entry)
            tracks.removeAll { $0.id == entry.id }
            await select(fallback)
        } catch { status = error.localizedDescription }
    }

    func save() {
        let frame = replay.frame()
        UserDefaults.standard.set(selectedTrackID, forKey: "selected-flight")
        desk.playbackTime = frame.elapsed
        desk.playbackRate = frame.rate
        do { UserDefaults.standard.set(try JSONEncoder().encode(desk), forKey: Self.storageKey) }
        catch { status = "Could not save the globe state: \(error.localizedDescription)" }
    }

    func record(rotation: simd_quatf, radius: Float) {
        desk.record(rotation: rotation, radius: radius)
        save()
    }
}
