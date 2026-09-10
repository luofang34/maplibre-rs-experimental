import SwiftUI
import simd
#if canImport(FlightExchange)
import FlightExchange
#endif

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
    private let library: FlightLibrary
    private let defaults: UserDefaults
    private var loadedLibrary = false
    var didRunLaunchActions = false
    private var selectionRevision: UInt64 = 0
    private var inboxSource: URL?

    @Published var controlsRequest: UInt64 = 0
    @Published var immersed = false
    @Published var opening = false
    @Published var status = ""
    var desk: DeskGlobeState
    let replay: FlightReplay

    init(library: FlightLibrary = FlightLibrary(), defaults: UserDefaults = .standard) {
        self.library = library
        self.defaults = defaults
        let restored = defaults.data(forKey: Self.storageKey)
            .flatMap { try? JSONDecoder().decode(DeskGlobeState.self, from: $0) } ?? .init()
        desk = restored
        replay = FlightReplay(restored: restored)
    }

    func loadLibrary() async {
        guard !loadedLibrary else { return }
        loadedLibrary = true
        do {
            tracks = try await library.entries()
            let id = defaults.string(forKey: "selected-flight") ?? "innsbruck-approach"
            if let entry = tracks.first(where: { $0.id == id }) ?? tracks.first {
                let time = desk.playbackTime, rate = desk.playbackRate
                await select(entry)
                replay.seek(time)
                replay.setRate(rate)
            } else { replay.replace(with: nil); selectedTrackID = "" }
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
            guard request == selectionRevision, tracks.contains(where: { $0.id == entry.id }) else { return }
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
            let fileName = url.lastPathComponent
            let name = inboxSource == url && fileName.count > 37 ? String(fileName.dropFirst(37)) : fileName
            let result = try await Task.detached(priority: .userInitiated) {
                let data = try FlightImport.read(url)
                return (data, try FlightImport.decode(data, name: name))
            }.value
            preview = ImportPreview(data: result.0, name: name, track: result.1)
        } catch { status = "Could not import this track: \(error.localizedDescription)" }
    }

    func receiveInbox() async {
        guard !importing, preview == nil else { return }
        do {
            guard let url = try FlightInbox().pending().first else { return }
            inboxSource = url
            await receive(url)
            if preview != nil { controlsRequest = controlsRequest &+ 1 }
            else { try FlightInbox().remove(url); inboxSource = nil }
        } catch { status = error.localizedDescription }
    }

    func cancelImport() {
        preview = nil
        if let url = inboxSource {
            do { try FlightInbox().remove(url); inboxSource = nil }
            catch { status = error.localizedDescription }
        }
    }

    func finishImport(elevation: FlightImport.Elevation, title: String) async {
        guard let preview, !importing else { return }
        importing = true
        defer { importing = false }
        do {
            let (entry, track) = try await library.importTrack(preview.data, name: preview.name, elevation: elevation, title: title)
            if !tracks.contains(entry) { tracks.append(entry) }
            selectionRevision = selectionRevision &+ 1
            replay.replace(with: track)
            selectedTrackID = entry.id
            cancelImport()
            status = "Imported \(entry.title). Press Fly track when ready."
            save()
        } catch { status = error.localizedDescription }
    }

    func remove(_ entry: FlightLibrary.Entry) async {
        do {
            try await library.remove(entry)
            tracks.removeAll { $0.id == entry.id }
            guard selectedTrackID == entry.id else { return }
            selectionRevision = selectionRevision &+ 1
            if let fallback = tracks.first { await select(fallback) }
            else { replay.replace(with: nil); selectedTrackID = ""; save() }
        } catch { status = error.localizedDescription }
    }

    func restoreDemos() async {
        do {
            try await library.restoreDemos()
            tracks = try await library.entries()
            if replay.track == nil, let first = tracks.first { await select(first) }
        } catch { status = error.localizedDescription }
    }

    func save() {
        let frame = replay.frame()
        defaults.set(selectedTrackID, forKey: "selected-flight")
        desk.playbackTime = frame.elapsed
        desk.playbackRate = frame.rate
        do { defaults.set(try JSONEncoder().encode(desk), forKey: Self.storageKey) }
        catch { status = "Could not save the globe state: \(error.localizedDescription)" }
    }

    func record(rotation: simd_quatf, radius: Float) {
        desk.record(rotation: rotation, radius: radius)
        save()
    }
}
