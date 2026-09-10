import Foundation
import CryptoKit

struct FlightImport {
    enum Elevation: String, CaseIterable {
        case msl = "EGM96 mean sea level"
        case ellipsoid = "WGS84 + EGM96 geoid"
    }
    enum Failure: LocalizedError {
        case message(String)
        var errorDescription: String? { switch self { case .message(let text): return text } }
    }
    struct Point {
        let date: Double
        let latitude: Double
        let longitude: Double
        let altitude: Double
        var geoid: Double?
        var startsSegment = false
    }
    static let byteLimit = 8 * 1024 * 1024
    static let pointLimit = 20_000

    static func read(_ url: URL) throws -> Data {
        guard url.isFileURL else { throw Failure.message("Open a local GPX, KML, or flight JSON file.") }
        let access = url.startAccessingSecurityScopedResource()
        defer { if access { url.stopAccessingSecurityScopedResource() } }
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let data = try handle.read(upToCount: byteLimit + 1) ?? Data()
        guard !data.isEmpty, data.count <= byteLimit else { throw Failure.message("Tracks must be between 1 byte and 8 MB.") }
        return data
    }

    static func decode(_ data: Data, name: String, elevation: Elevation = .msl) throws -> FlightTrack {
        guard data.count <= byteLimit else { throw Failure.message("This track exceeds the 8 MB limit.") }
        let suffix = URL(fileURLWithPath: name).pathExtension.lowercased()
        if suffix == "json" || suffix == "flighttrack" { return try FlightTrack.decode(data) }
        guard ["gpx", "kml"].contains(suffix) else { throw Failure.message("Use a GPX track, timestamped KML gx:Track, or flight JSON.") }
        let points = try FlightXML.decode(data, format: suffix)
        let datum: Elevation = suffix == "kml" ? .msl : elevation
        let observations = try normalized(points, elevation: datum)
        let title = String(URL(fileURLWithPath: name).deletingPathExtension().lastPathComponent.prefix(120))
        let source = FlightTrack.Provenance(url: "", license: "User-imported recording; source rights remain with its owner.",
            sha256: SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined(),
            altitudeConversion: datum == .msl ? "Source elevations declared as metres above EGM96 mean sea level." : "MSL = WGS84 ellipsoid elevation minus the GPX geoidheight field, declared as EGM96.",
            coverage: "Imported \(suffix.uppercased()) recording. Segment breaks and gaps longer than 20 seconds remain unavailable. Ground speed, true track and vertical speed are derived from positions; aircraft attitude is unavailable.")
        let track = FlightTrack(title: title, kind: .recorded, callsign: title, registration: "Imported",
            aircraft: "Track recording", destination: "", startUTC: points[0].date, source: source, observations: observations)
        return try FlightTrack.decode(JSONEncoder().encode(track))
    }

    private static func normalized(_ points: [Point], elevation: Elevation) throws -> [FlightTrack.Observation] {
        guard (2...pointLimit).contains(points.count) else { throw Failure.message("A flight needs 2–20,000 timestamped positions with altitude.") }
        var observations: [FlightTrack.Observation] = []
        for (i, point) in points.enumerated() {
            guard point.date.isFinite, point.latitude.isFinite, abs(point.latitude) <= 84,
                  point.longitude.isFinite, abs(point.longitude) <= 180, point.altitude.isFinite else {
                throw Failure.message("Position \(i + 1) contains an invalid coordinate or altitude.")
            }
            if i > 0, point.date <= points[i - 1].date { throw Failure.message("Track timestamps must increase; duplicate or reversed timestamps cannot be replayed.") }
            let geoid = elevation == .ellipsoid ? point.geoid : nil
            if elevation == .ellipsoid, geoid == nil { throw Failure.message("WGS84 elevation needs geoidheight at every GPX point. Export MSL heights or include the geoid correction.") }
            let altitude = point.altitude - (geoid ?? 0)
            let other: Point?
            if i + 1 < points.count, !points[i + 1].startsSegment, points[i + 1].date - point.date <= 20 {
                other = points[i + 1]
            } else if i > 0, !point.startsSegment, point.date - points[i - 1].date <= 20 {
                other = points[i - 1]
            } else { other = nil }
            let velocity = other.map { point.date < $0.date ? Self.velocity(point, $0) : Self.velocity($0, point) }
            guard velocity?.speed ?? 0 <= 500 else { throw Failure.message("Position \(i + 1) jumps faster than 500 m/s. Split receiver discontinuities into separate segments.") }
            observations.append(.init(time: point.date - points[0].date, latitude: point.latitude, longitude: point.longitude,
                altitudeMSL: altitude, altitudeGNSS: geoid.map { _ in point.altitude }, geoidSeparation: geoid,
                groundSpeed: velocity?.speed ?? 0, track: velocity?.bearing ?? 0, roll: nil, startsSegment: point.startsSegment, velocityAvailable: velocity != nil))
        }
        return observations
    }

    private static func velocity(_ a: Point, _ b: Point) -> (speed: Double, bearing: Double) {
        let lat1 = a.latitude * .pi / 180, lat2 = b.latitude * .pi / 180
        let dlat = lat2 - lat1, dlon = (b.longitude - a.longitude) * .pi / 180
        let h = pow(sin(dlat / 2), 2) + cos(lat1) * cos(lat2) * pow(sin(dlon / 2), 2)
        let distance = 2 * MapPlacement.earthRadiusMeters * asin(sqrt(min(max(h, 0), 1)))
        let bearing = atan2(sin(dlon) * cos(lat2), cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dlon))
        return (distance / (b.date - a.date), (bearing * 180 / .pi + 360).truncatingRemainder(dividingBy: 360))
    }
}
