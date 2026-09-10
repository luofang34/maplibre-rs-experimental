import Foundation

/// Bounded file handoff between the Share extension and the application; no renderer dependencies.
struct FlightInbox {
    static let group = "group.com.sokolysystems.maplibre.vision"
    static let byteLimit = 8 * 1024 * 1024
    static let extensions = ["gpx", "kml", "json", "flighttrack"]
    enum Failure: LocalizedError {
        case unavailable, unsupported, full, oversized
        var errorDescription: String? {
            switch self {
            case .unavailable: return "The shared flight inbox is unavailable. Open the file with MapLibre Vision instead."
            case .unsupported: return "Choose a GPX, KML, or flight JSON recording."
            case .full: return "Open MapLibre Vision to finish importing the flights in its inbox."
            case .oversized: return "Track files must be between 1 byte and 8 MB."
            }
        }
    }
    let directory: URL

    init(directory: URL? = nil) throws {
        guard let root = directory ?? FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: Self.group)?.appendingPathComponent("IncomingFlights", isDirectory: true) else { throw Failure.unavailable }
        self.directory = root
    }

    func pending() throws -> [URL] {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return try FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)
            .filter { Self.extensions.contains($0.pathExtension.lowercased()) }.sorted { $0.lastPathComponent < $1.lastPathComponent }
    }

    static func fileName(source: URL, suggested: String?, contentType: String) throws -> String {
        for name in [suggested, source.lastPathComponent].compactMap({ $0 }) {
            if extensions.contains(URL(fileURLWithPath: name).pathExtension.lowercased()) { return name }
        }
        let types = ["com.google.earth.kml": "kml", "org.topografix.gpx": "gpx",
                     "com.sokolysystems.flight-track": "flighttrack", "public.json": "json"]
        guard let suffix = types[contentType] else { throw Failure.unsupported }
        let stem = suggested.flatMap { $0.isEmpty ? nil : $0 } ?? "Flight"
        return String(stem.prefix(100)) + "." + suffix
    }

    func save(_ source: URL, name: String? = nil) throws {
        let name = String((name ?? source.lastPathComponent).prefix(160))
        guard Self.extensions.contains(URL(fileURLWithPath: name).pathExtension.lowercased()) else { throw Failure.unsupported }
        guard try pending().count < 10 else { throw Failure.full }
        let access = source.startAccessingSecurityScopedResource()
        defer { if access { source.stopAccessingSecurityScopedResource() } }
        let file = try FileHandle(forReadingFrom: source)
        defer { try? file.close() }
        let bytes = try file.read(upToCount: Self.byteLimit + 1) ?? Data()
        guard !bytes.isEmpty, bytes.count <= Self.byteLimit else { throw Failure.oversized }
        let safeName = name.replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: ":", with: "_")
        try bytes.write(to: directory.appendingPathComponent(UUID().uuidString + "_" + safeName), options: .atomic)
    }

    func remove(_ url: URL) throws {
        guard url.deletingLastPathComponent().standardizedFileURL == directory.standardizedFileURL else { throw Failure.unsupported }
        try FileManager.default.removeItem(at: url)
    }
}
