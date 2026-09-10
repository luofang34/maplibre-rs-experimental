import Foundation

actor FlightLibrary {
    struct Entry: Identifiable, Equatable {
        let id: String
        let title: String
        let simulated: Bool
        let imported: Bool
    }
    private let directory: URL
    private let bundle: Bundle
    static let capacity = 20
    static let demos = ["innsbruck-approach", "mach-loop", "liberty"]

    private var hiddenURL: URL { directory.appendingPathComponent("hidden-demos.json") }

    private func hiddenDemos() throws -> Set<String> {
        guard FileManager.default.fileExists(atPath: hiddenURL.path) else { return [] }
        return try JSONDecoder().decode(Set<String>.self, from: FlightImport.read(hiddenURL))
    }

    init(directory: URL? = nil, bundle: Bundle = .main) {
        self.directory = directory ?? FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("FlightTracks", isDirectory: true)
        self.bundle = bundle
    }

    func entries() throws -> [Entry] {
        var entries: [Entry] = []
        let hidden = try hiddenDemos()
        for name in Self.demos where !hidden.contains(name) {
            let entry = Entry(id: name, title: name, simulated: false, imported: false)
            if let track = try? load(entry) {
                entries.append(.init(id: name, title: track.displayTitle, simulated: track.isSimulation, imported: false))
            }
        }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let files = try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "flighttrack" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
        guard files.count <= Self.capacity else { throw FlightImport.Failure.message("The flight library exceeds its 20-recording limit.") }
        for url in files {
            let track = try FlightTrack.decode(FlightImport.read(url))
            entries.append(.init(id: url.deletingPathExtension().lastPathComponent,
                                 title: track.displayTitle, simulated: track.isSimulation, imported: true))
        }
        return entries
    }

    func load(_ entry: Entry) throws -> FlightTrack {
        let url: URL
        if entry.imported {
            guard UUID(uuidString: entry.id) != nil else { throw FlightImport.Failure.message("Invalid recording identifier.") }
            url = directory.appendingPathComponent(entry.id).appendingPathExtension("flighttrack")
        } else {
            guard Self.demos.contains(entry.id),
                  let resource = bundle.url(forResource: entry.id, withExtension: entry.id == "liberty" ? "kml" : "json") else {
                throw FlightImport.Failure.message("This bundled recording is unavailable.")
            }
            url = resource
        }
        var track = try FlightImport.decode(FlightImport.read(url), name: url.lastPathComponent)
        if entry.id == "liberty" { track.title = "Liberty" }
        return track
    }

    func importTrack(_ data: Data, name: String, elevation: FlightImport.Elevation, title: String? = nil) throws -> (Entry, FlightTrack) {
        var track = try FlightImport.decode(data, name: name, elevation: elevation)
        if let title, !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { track.title = String(title.prefix(120)) }
        let existing = try entries().filter(\.imported)
        for entry in existing {
            let saved = try load(entry)
            if saved.source.sha256 == track.source.sha256,
               saved.source.altitudeConversion == track.source.altitudeConversion { return (entry, saved) }
        }
        guard existing.count < Self.capacity else { throw FlightImport.Failure.message("Your library has 20 imported flights. Remove one before importing another.") }
        let entry = Entry(id: UUID().uuidString, title: track.displayTitle, simulated: track.isSimulation, imported: true)
        try JSONEncoder().encode(track).write(to: directory.appendingPathComponent(entry.id).appendingPathExtension("flighttrack"), options: .atomic)
        return (entry, track)
    }

    func restoreDemos() throws {
        if FileManager.default.fileExists(atPath: hiddenURL.path) { try FileManager.default.removeItem(at: hiddenURL) }
    }

    func remove(_ entry: Entry) throws {
        if !entry.imported {
            guard Self.demos.contains(entry.id) else { return }
            var hidden = try hiddenDemos()
            hidden.insert(entry.id)
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try JSONEncoder().encode(hidden).write(to: hiddenURL, options: .atomic)
            return
        }
        guard UUID(uuidString: entry.id) != nil else { return }
        try FileManager.default.removeItem(at: directory.appendingPathComponent(entry.id).appendingPathExtension("flighttrack"))
    }
}
